use std::{
    collections::BTreeMap,
    env,
    fmt::Display,
    fs,
    io::{BufReader, Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Result, anyhow};
use nautilus_model::enums::{
    OmsType, OrderSide, OrderType, PositionSide, TimeInForce, TrailingOffsetType, TriggerType,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_config::{LoadedBoltV3Config, RESOLUTION_GATE_ROLE, load_bolt_v3_config},
    bolt_v3_decision_evidence::{
        BoltV3GateEvidenceIdentity, BoltV3ReadinessGateEvidenceSnapshot,
        BoltV3StrategyInputEvidenceSnapshot, validate_readiness_gate_evidence_snapshot,
    },
    bolt_v3_live_canary_gate::{
        BoltV3LiveCanaryGateError, check_bolt_v3_live_canary_pre_consumption_gate,
    },
    bolt_v3_market_families::{
        MarketSelectionCandidateWindow, MarketSelectionOutcome, SelectedBinaryOptionMarket,
    },
    bolt_v3_no_submit_readiness_schema::{
        APPROVAL_CONSUMPTION_RECORD_KIND, APPROVAL_CONSUMPTION_SCHEMA_VERSION,
    },
};

const PHASE8_CANARY_EVIDENCE_SCHEMA_VERSION: u32 = 1;
const SUBMIT_ADMISSION_STATUS_ACCEPTED: &str = "accepted";
const SUBMIT_ADMISSION_STATUS_REJECTED: &str = "rejected";
const NT_ADAPTER_SUBMIT_PROVEN_REASON: &str = "nt_adapter_submit_proven";
const BLOCKED_BEFORE_LIVE_ORDER_REASON: &str = "blocked_before_live_order";
const BLOCKED_BEFORE_SUBMIT_REASON: &str = "blocked_before_submit";
const PHASE8_REQUIRED_LIVE_ORDER_CAP: u32 = 1;
const PHASE8_SHA256_BUFFER_BYTES: usize = 8 * 1024;
pub const PHASE8_MARKET_SELECTION_OUTCOME_CURRENT: &str = "current";
pub const PHASE8_MARKET_SELECTION_OUTCOME_NEXT: &str = "next";
pub const PHASE8_MARKET_SELECTION_SOURCE_RECORD_KIND: &str = "market_selection_result";
pub const PHASE8_MARKET_SELECTION_SOURCE: &str = "nt_runtime_selection_snapshot";
pub const PHASE8_BLOCKED_BEFORE_LIVE_RUNNER_RUN_ID: &str = "phase8-blocked-before-live-runner";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase8CanaryPreflightStatus {
    Missing,
    AcceptedByGate,
    RejectedByGate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase8StrategyInputAuditStatus {
    Approved,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase8CanaryBlockReason {
    MissingNoSubmitReadinessReport,
    LiveCanaryGateRejected,
    StrategyInputSafetyAuditBlocked,
    LiveOrderCountCapNotOne,
    NonPositiveRealizedVolatility,
    NonPositiveTimeToMarketEnd,
    NonPositiveSpotPrice,
    NonPositivePriceToBeatValue,
    NonPositiveExpectedEdgeBasisPoints,
    NonPositiveWorstCaseEdgeBasisPoints,
    EdgeBasisPointsMismatch,
    NonPositiveThetaScaledMinEdgeBps,
    NegativeFeeRateBasisPoints,
    MissingPriceToBeatSource,
    UnsupportedPriceToBeatSource,
    MissingReferenceQuoteTsEvent,
    InvalidPricingKurtosis,
    NegativeThetaDecayFactor,
    MissingSelectedMarketIdentity,
    InvalidMarketSelectionOutcome,
    InvalidMarketSelectionBinding,
    InvalidSelectedMarketWindow,
    DecisionEvidenceUnavailable,
    RuntimeNoAdmittedOrder,
    BlockedBeforeLiveOrder,
    RootConfigHashUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8StrategyInputSafetyAudit {
    status: Phase8StrategyInputAuditStatus,
    block_reasons: Vec<Phase8CanaryBlockReason>,
}

pub struct Phase8StrategyInputSafetyInputs<'a> {
    pub realized_volatility: Decimal,
    pub seconds_to_market_end: u64,
    pub spot_price: Decimal,
    pub price_to_beat_value: Decimal,
    pub expected_edge_basis_points: Decimal,
    pub worst_case_edge_basis_points: Decimal,
    pub theta_scaled_min_edge_bps: Decimal,
    pub fee_rate_basis_points: Decimal,
    pub price_to_beat_source: &'a str,
    pub expected_price_to_beat_source: &'a str,
    pub reference_quote_ts_event: u64,
    pub pricing_kurtosis: Decimal,
    pub theta_decay_factor: Decimal,
}

impl Phase8StrategyInputSafetyAudit {
    pub fn approved() -> Self {
        Self {
            status: Phase8StrategyInputAuditStatus::Approved,
            block_reasons: Vec::new(),
        }
    }

    pub fn blocked(block_reasons: Vec<Phase8CanaryBlockReason>) -> Self {
        Self {
            status: Phase8StrategyInputAuditStatus::Blocked,
            block_reasons,
        }
    }

    pub fn from_strategy_inputs(inputs: Phase8StrategyInputSafetyInputs<'_>) -> Self {
        let mut block_reasons = Vec::new();
        if inputs.realized_volatility <= Decimal::ZERO {
            block_reasons.push(Phase8CanaryBlockReason::NonPositiveRealizedVolatility);
        }
        if inputs.seconds_to_market_end == 0 {
            block_reasons.push(Phase8CanaryBlockReason::NonPositiveTimeToMarketEnd);
        }
        if inputs.spot_price <= Decimal::ZERO {
            block_reasons.push(Phase8CanaryBlockReason::NonPositiveSpotPrice);
        }
        if inputs.price_to_beat_value <= Decimal::ZERO {
            block_reasons.push(Phase8CanaryBlockReason::NonPositivePriceToBeatValue);
        }
        if inputs.expected_edge_basis_points <= Decimal::ZERO {
            block_reasons.push(Phase8CanaryBlockReason::NonPositiveExpectedEdgeBasisPoints);
        }
        if inputs.worst_case_edge_basis_points <= Decimal::ZERO {
            block_reasons.push(Phase8CanaryBlockReason::NonPositiveWorstCaseEdgeBasisPoints);
        }
        if inputs.expected_edge_basis_points != inputs.worst_case_edge_basis_points {
            block_reasons.push(Phase8CanaryBlockReason::EdgeBasisPointsMismatch);
        }
        if inputs.theta_scaled_min_edge_bps <= Decimal::ZERO {
            block_reasons.push(Phase8CanaryBlockReason::NonPositiveThetaScaledMinEdgeBps);
        }
        if inputs.fee_rate_basis_points < Decimal::ZERO {
            block_reasons.push(Phase8CanaryBlockReason::NegativeFeeRateBasisPoints);
        }
        let price_to_beat_source = inputs.price_to_beat_source.trim();
        let expected_price_to_beat_source = inputs.expected_price_to_beat_source.trim();
        if price_to_beat_source.is_empty() {
            block_reasons.push(Phase8CanaryBlockReason::MissingPriceToBeatSource);
        } else if expected_price_to_beat_source.is_empty()
            || price_to_beat_source != expected_price_to_beat_source
        {
            block_reasons.push(Phase8CanaryBlockReason::UnsupportedPriceToBeatSource);
        }
        if inputs.reference_quote_ts_event == 0 {
            block_reasons.push(Phase8CanaryBlockReason::MissingReferenceQuoteTsEvent);
        }
        if inputs.pricing_kurtosis <= Decimal::new(-6, 0) {
            block_reasons.push(Phase8CanaryBlockReason::InvalidPricingKurtosis);
        }
        if inputs.theta_decay_factor < Decimal::ZERO {
            block_reasons.push(Phase8CanaryBlockReason::NegativeThetaDecayFactor);
        }
        if block_reasons.is_empty() {
            Self::approved()
        } else {
            Self::blocked(block_reasons)
        }
    }

    pub fn from_evidence_file(
        path: impl AsRef<Path>,
        expected_sha256: impl AsRef<str>,
        expected_price_to_beat_source: impl AsRef<str>,
    ) -> Result<Self> {
        let path = path.as_ref();
        let expected_sha256 = expected_sha256.as_ref().trim();
        if expected_sha256.is_empty() {
            return Err(anyhow!(
                "required phase8 strategy input evidence sha256 is empty"
            ));
        }
        let raw: Phase8StrategyInputEvidenceFile =
            Phase8OperatorApprovalEnvelope::read_json_file_with_expected_sha256(
                path,
                expected_sha256,
                "phase8 strategy input evidence",
                "phase8 strategy input evidence sha256 does not match current evidence",
            )?;
        Self::from_raw_evidence(raw, expected_price_to_beat_source.as_ref(), None)
    }

    pub fn from_evidence_bytes_with_market_selection_source(
        evidence_bytes: &[u8],
        expected_sha256: impl AsRef<str>,
        expected_price_to_beat_source: impl AsRef<str>,
        market_selection_source_bytes: &[u8],
    ) -> Result<Self> {
        let expected_sha256 = expected_sha256.as_ref().trim();
        if expected_sha256.is_empty() {
            return Err(anyhow!(
                "required phase8 strategy input evidence sha256 is empty"
            ));
        }
        if Phase8OperatorApprovalEnvelope::sha256_bytes(evidence_bytes) != expected_sha256 {
            return Err(anyhow!(
                "phase8 strategy input evidence sha256 does not match current evidence"
            ));
        }
        let raw: Phase8StrategyInputEvidenceFile =
            serde_json::from_slice(evidence_bytes).map_err(|source| {
                anyhow!("failed to parse phase8 strategy input evidence: {source}")
            })?;
        Self::from_raw_evidence(
            raw,
            expected_price_to_beat_source.as_ref(),
            Some(market_selection_source_bytes),
        )
    }

    fn from_raw_evidence(
        raw: Phase8StrategyInputEvidenceFile,
        expected_price_to_beat_source: &str,
        market_selection_source_bytes: Option<&[u8]>,
    ) -> Result<Self> {
        let realized_volatility =
            Decimal::from_str_exact(raw.realized_volatility.trim()).map_err(|source| {
                anyhow!("failed to parse phase8 strategy input realized_volatility: {source}")
            })?;
        let spot_price = Decimal::from_str_exact(raw.spot_price.trim()).map_err(|source| {
            anyhow!("failed to parse phase8 strategy input spot_price: {source}")
        })?;
        let price_to_beat_value =
            Decimal::from_str_exact(raw.price_to_beat_value.trim()).map_err(|source| {
                anyhow!("failed to parse phase8 strategy input price_to_beat_value: {source}")
            })?;
        let expected_edge_basis_points =
            Decimal::from_str_exact(raw.expected_edge_basis_points.trim()).map_err(|source| {
                anyhow!(
                    "failed to parse phase8 strategy input expected_edge_basis_points: {source}"
                )
            })?;
        let worst_case_edge_basis_points =
            Decimal::from_str_exact(raw.worst_case_edge_basis_points.trim()).map_err(|source| {
                anyhow!(
                    "failed to parse phase8 strategy input worst_case_edge_basis_points: {source}"
                )
            })?;
        let fee_rate_basis_points = Decimal::from_str_exact(raw.fee_rate_basis_points.trim())
            .map_err(|source| {
                anyhow!("failed to parse phase8 strategy input fee_rate_basis_points: {source}")
            })?;
        let pricing_kurtosis =
            Decimal::from_str_exact(raw.pricing_kurtosis.trim()).map_err(|source| {
                anyhow!("failed to parse phase8 strategy input pricing_kurtosis: {source}")
            })?;
        let theta_decay_factor =
            Decimal::from_str_exact(raw.theta_decay_factor.trim()).map_err(|source| {
                anyhow!("failed to parse phase8 strategy input theta_decay_factor: {source}")
            })?;
        let theta_scaled_min_edge_bps =
            Decimal::from_str_exact(raw.theta_scaled_min_edge_bps.trim()).map_err(|source| {
                anyhow!("failed to parse phase8 strategy input theta_scaled_min_edge_bps: {source}")
            })?;
        let readiness_identity_valid = phase8_strategy_input_readiness_identity_valid(&raw);
        // The caller-supplied `expected_price_to_beat_source` is config-derived and MUST flow
        // straight through so `from_strategy_inputs` keeps a genuine config-vs-runtime equality
        // check (raw.price_to_beat_source vs config). Never reassign it from the evidence file's
        // own raw value — doing so turns the integrity check into a self-comparison that always
        // passes. `readiness_identity_valid` is an additional fail-closed gate below
        // (DecisionEvidenceUnavailable), never a license to drop the price-to-beat binding.
        let mut audit = Self::from_strategy_inputs(Phase8StrategyInputSafetyInputs {
            realized_volatility,
            seconds_to_market_end: raw.seconds_to_market_end,
            spot_price,
            price_to_beat_value,
            expected_edge_basis_points,
            worst_case_edge_basis_points,
            theta_scaled_min_edge_bps,
            fee_rate_basis_points,
            price_to_beat_source: &raw.price_to_beat_source,
            expected_price_to_beat_source,
            reference_quote_ts_event: raw.reference_quote_ts_event,
            pricing_kurtosis,
            theta_decay_factor,
        });
        audit.block_if(
            !readiness_identity_valid,
            Phase8CanaryBlockReason::DecisionEvidenceUnavailable,
        );
        let market_selection_outcome = raw.market_selection_outcome.trim();
        audit.block_if(
            market_selection_outcome.is_empty()
                || raw.polymarket_condition_id.trim().is_empty()
                || raw.polymarket_market_slug.trim().is_empty()
                || raw.polymarket_question_id.trim().is_empty()
                || raw.up_instrument_id.trim().is_empty()
                || raw.down_instrument_id.trim().is_empty(),
            Phase8CanaryBlockReason::MissingSelectedMarketIdentity,
        );
        audit.block_if(
            !market_selection_outcome.is_empty()
                && !phase8_market_selection_outcome_is_live_entry_candidate(
                    market_selection_outcome,
                ),
            Phase8CanaryBlockReason::InvalidMarketSelectionOutcome,
        );
        let source_bound_candidate_market_start_timestamps_ms =
            phase8_source_bound_candidate_market_start_timestamps(
                &raw,
                market_selection_outcome,
                market_selection_source_bytes,
            )?;
        audit.block_if(
            phase8_market_selection_outcome_is_live_entry_candidate(market_selection_outcome)
                && source_bound_candidate_market_start_timestamps_ms.is_none(),
            Phase8CanaryBlockReason::InvalidMarketSelectionBinding,
        );
        let candidate_market_start_timestamps_ms = match market_selection_outcome {
            PHASE8_MARKET_SELECTION_OUTCOME_NEXT => {
                source_bound_candidate_market_start_timestamps_ms
                    .as_deref()
                    .unwrap_or(&[])
            }
            _ => raw
                .candidate_market_start_timestamps_ms
                .as_deref()
                .unwrap_or(&[]),
        };
        audit.block_if(
            !market_selection_outcome.is_empty()
                && !phase8_market_selection_outcome_matches_window(
                    market_selection_outcome,
                    raw.market_selection_timestamp_ms,
                    raw.polymarket_market_start_timestamp_ms,
                    raw.polymarket_market_end_timestamp_ms,
                    candidate_market_start_timestamps_ms,
                ),
            Phase8CanaryBlockReason::InvalidMarketSelectionBinding,
        );
        audit.block_if(
            raw.selected_market_observed_timestamp_ms == u64::MIN
                || raw.market_selection_timestamp_ms == u64::MIN
                || raw.polymarket_market_start_timestamp_ms == u64::MIN
                || raw.polymarket_market_end_timestamp_ms
                    <= raw.polymarket_market_start_timestamp_ms,
            Phase8CanaryBlockReason::InvalidSelectedMarketWindow,
        );
        Ok(audit)
    }

    pub fn is_approved(&self) -> bool {
        self.status == Phase8StrategyInputAuditStatus::Approved
    }

    pub fn block_reasons(&self) -> &[Phase8CanaryBlockReason] {
        &self.block_reasons
    }

    fn block_if(&mut self, condition: bool, reason: Phase8CanaryBlockReason) {
        if condition {
            self.status = Phase8StrategyInputAuditStatus::Blocked;
            self.block_reasons.push(reason);
        }
    }
}

fn phase8_source_bound_candidate_market_start_timestamps(
    raw: &Phase8StrategyInputEvidenceFile,
    market_selection_outcome: &str,
    market_selection_source_bytes: Option<&[u8]>,
) -> Result<Option<Vec<u64>>> {
    if !phase8_market_selection_outcome_is_live_entry_candidate(market_selection_outcome) {
        return Ok(None);
    }
    let Some(source_path) = raw
        .market_selection_source_path
        .as_deref()
        .map(str::trim)
        .filter(|source_path| !source_path.is_empty())
    else {
        return Ok(None);
    };
    let Some(source_sha256) = raw
        .market_selection_source_sha256
        .as_deref()
        .map(str::trim)
        .filter(|source_sha256| phase8_is_sha256_hex(source_sha256))
    else {
        return Ok(None);
    };

    phase8_reject_parent_dir(source_path, "market selection source evidence")?;
    let source: Phase8MarketSelectionSourceEvidenceFile =
        if let Some(source_bytes) = market_selection_source_bytes {
            if Phase8OperatorApprovalEnvelope::sha256_bytes(source_bytes) != source_sha256 {
                return Ok(None);
            }
            serde_json::from_slice(source_bytes).map_err(|source| {
                anyhow!("failed to parse phase8 market selection source evidence: {source}")
            })?
        } else {
            Phase8OperatorApprovalEnvelope::read_json_file_with_expected_sha256(
                source_path,
                source_sha256,
                "phase8 market selection source evidence",
                "phase8 market selection source evidence sha256 does not match current evidence",
            )?
        };

    if source.record_kind.trim() != PHASE8_MARKET_SELECTION_SOURCE_RECORD_KIND
        || source.source.trim() != PHASE8_MARKET_SELECTION_SOURCE
        || !phase8_market_selection_source_matches_strategy(raw, &source)
    {
        return Ok(None);
    }
    if market_selection_outcome == PHASE8_MARKET_SELECTION_OUTCOME_NEXT
        && let Some(reported_candidates) = raw.candidate_market_start_timestamps_ms.as_ref()
        && reported_candidates != &source.candidate_market_start_timestamps_ms
    {
        return Ok(None);
    }

    Ok(Some(source.candidate_market_start_timestamps_ms))
}

fn phase8_market_selection_source_matches_strategy(
    raw: &Phase8StrategyInputEvidenceFile,
    source: &Phase8MarketSelectionSourceEvidenceFile,
) -> bool {
    source.market_selection_timestamp_ms == raw.market_selection_timestamp_ms
        && source.market_selection_outcome.trim() == raw.market_selection_outcome.trim()
        && source.polymarket_condition_id.trim() == raw.polymarket_condition_id.trim()
        && source.polymarket_market_slug.trim() == raw.polymarket_market_slug.trim()
        && source.polymarket_question_id.trim() == raw.polymarket_question_id.trim()
        && source.up_instrument_id.trim() == raw.up_instrument_id.trim()
        && source.down_instrument_id.trim() == raw.down_instrument_id.trim()
        && source.selected_market_observed_timestamp_ms == raw.selected_market_observed_timestamp_ms
        && source.polymarket_market_start_timestamp_ms == raw.polymarket_market_start_timestamp_ms
        && source.polymarket_market_end_timestamp_ms == raw.polymarket_market_end_timestamp_ms
}

fn phase8_market_selection_outcome_is_live_entry_candidate(outcome: &str) -> bool {
    outcome == PHASE8_MARKET_SELECTION_OUTCOME_CURRENT
        || outcome == PHASE8_MARKET_SELECTION_OUTCOME_NEXT
}

fn phase8_market_selection_outcome_matches_window(
    outcome: &str,
    market_selection_timestamp_ms: u64,
    market_start_timestamp_ms: u64,
    market_end_timestamp_ms: u64,
    candidate_market_start_timestamps_ms: &[u64],
) -> bool {
    match outcome {
        PHASE8_MARKET_SELECTION_OUTCOME_CURRENT => {
            market_start_timestamp_ms <= market_selection_timestamp_ms
                && market_selection_timestamp_ms < market_end_timestamp_ms
        }
        PHASE8_MARKET_SELECTION_OUTCOME_NEXT => phase8_market_selection_start_is_nearest_next(
            market_selection_timestamp_ms,
            market_start_timestamp_ms,
            candidate_market_start_timestamps_ms,
        ),
        _ => false,
    }
}

fn phase8_market_selection_start_is_nearest_next(
    market_selection_timestamp_ms: u64,
    market_start_timestamp_ms: u64,
    candidate_market_start_timestamps_ms: &[u64],
) -> bool {
    candidate_market_start_timestamps_ms
        .iter()
        .copied()
        .filter(|candidate_start_timestamp_ms| {
            *candidate_start_timestamp_ms > market_selection_timestamp_ms
        })
        .min()
        == Some(market_start_timestamp_ms)
}

fn phase8_strategy_input_readiness_identity_valid(raw: &Phase8StrategyInputEvidenceFile) -> bool {
    let Some(gate_session_hash) = raw.gate_session_hash.as_deref() else {
        return false;
    };
    let Some(selected_market_key) = raw.selected_market_key.as_deref() else {
        return false;
    };
    let Some(gate_evidence) = raw.gate_evidence.as_ref() else {
        return false;
    };

    validate_readiness_gate_evidence_snapshot(&BoltV3ReadinessGateEvidenceSnapshot {
        gate_session_hash: gate_session_hash.to_string(),
        selected_market_key: selected_market_key.to_string(),
        gate_evidence: gate_evidence.clone(),
    })
    .is_ok()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phase8StrategyInputEvidenceFile {
    strategy_instance_id: Option<String>,
    realized_volatility: String,
    seconds_to_market_end: u64,
    spot_price: String,
    price_to_beat_value: String,
    expected_edge_basis_points: String,
    worst_case_edge_basis_points: String,
    fee_rate_basis_points: String,
    price_to_beat_source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    gate_session_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_market_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    gate_evidence: Option<BTreeMap<String, BoltV3GateEvidenceIdentity>>,
    reference_quote_ts_event: u64,
    pricing_kurtosis: String,
    theta_decay_factor: String,
    theta_scaled_min_edge_bps: String,
    market_selection_timestamp_ms: u64,
    candidate_market_start_timestamps_ms: Option<Vec<u64>>,
    market_selection_source_path: Option<String>,
    market_selection_source_sha256: Option<String>,
    market_selection_outcome: String,
    polymarket_condition_id: String,
    polymarket_market_slug: String,
    polymarket_question_id: String,
    up_instrument_id: String,
    down_instrument_id: String,
    selected_market_observed_timestamp_ms: u64,
    polymarket_market_start_timestamp_ms: u64,
    polymarket_market_end_timestamp_ms: u64,
}

impl Phase8StrategyInputEvidenceFile {
    pub fn from_runtime_snapshot_and_market_selection_source(
        snapshot: &BoltV3StrategyInputEvidenceSnapshot,
        strategy_instance_id: impl AsRef<str>,
        runtime_strategy_id: impl AsRef<str>,
        market_selection_source: &Phase8MarketSelectionSourceEvidenceFile,
        market_selection_source_path: impl AsRef<str>,
        market_selection_source_sha256: impl AsRef<str>,
        candidate_market_start_timestamps_ms: &[u64],
    ) -> Result<Self> {
        let strategy_instance_id = strategy_instance_id.as_ref().trim();
        if strategy_instance_id.is_empty() {
            return Err(anyhow!(
                "phase8 strategy input evidence requires strategy_instance_id"
            ));
        }
        let runtime_strategy_id = runtime_strategy_id.as_ref().trim();
        if runtime_strategy_id.is_empty() {
            return Err(anyhow!(
                "phase8 strategy input evidence requires runtime_strategy_id"
            ));
        }
        if snapshot.strategy_id != runtime_strategy_id {
            return Err(anyhow!(
                "phase8 strategy input evidence runtime strategy_id does not match config"
            ));
        }
        let market_selection_timestamp_ms =
            snapshot.market_selection_timestamp_ms.ok_or_else(|| {
                anyhow!("phase8 strategy input evidence requires market_selection_timestamp_ms")
            })?;
        let selected_market_observed_timestamp_ms = snapshot
            .selected_market_observed_timestamp_ms
            .ok_or_else(|| {
                anyhow!(
                    "phase8 strategy input evidence requires selected_market_observed_timestamp_ms"
                )
            })?;
        let polymarket_market_start_timestamp_ms = snapshot
            .polymarket_market_start_timestamp_ms
            .ok_or_else(|| {
                anyhow!(
                    "phase8 strategy input evidence requires polymarket_market_start_timestamp_ms"
                )
            })?;
        let polymarket_market_end_timestamp_ms =
            snapshot.polymarket_market_end_timestamp_ms.ok_or_else(|| {
                anyhow!(
                    "phase8 strategy input evidence requires polymarket_market_end_timestamp_ms"
                )
            })?;
        let source_path = market_selection_source_path.as_ref().trim();
        if source_path.is_empty() {
            return Err(anyhow!(
                "phase8 strategy input evidence requires market_selection_source_path"
            ));
        }
        let source_sha256 = market_selection_source_sha256.as_ref().trim();
        if !phase8_is_sha256_hex(source_sha256) {
            return Err(anyhow!(
                "phase8 strategy input evidence requires market_selection_source_sha256"
            ));
        }

        let raw = Self {
            strategy_instance_id: Some(strategy_instance_id.to_string()),
            realized_volatility: snapshot.realized_volatility.clone(),
            seconds_to_market_end: snapshot.seconds_to_market_end,
            spot_price: snapshot.spot_price.clone(),
            price_to_beat_value: snapshot.price_to_beat_value.clone(),
            expected_edge_basis_points: snapshot.expected_edge_basis_points.clone(),
            worst_case_edge_basis_points: snapshot.worst_case_edge_basis_points.clone(),
            fee_rate_basis_points: snapshot.fee_rate_basis_points.clone(),
            price_to_beat_source: snapshot.price_to_beat_source.clone(),
            gate_session_hash: Some(snapshot.gate_session_hash.clone()),
            selected_market_key: Some(snapshot.selected_market_key.clone()),
            gate_evidence: Some(snapshot.gate_evidence.clone()),
            reference_quote_ts_event: snapshot.reference_quote_ts_event,
            pricing_kurtosis: snapshot.pricing_kurtosis.clone(),
            theta_decay_factor: snapshot.theta_decay_factor.clone(),
            theta_scaled_min_edge_bps: snapshot.theta_scaled_min_edge_bps.clone(),
            market_selection_timestamp_ms,
            candidate_market_start_timestamps_ms: Some(
                candidate_market_start_timestamps_ms.to_vec(),
            ),
            market_selection_source_path: Some(source_path.to_string()),
            market_selection_source_sha256: Some(source_sha256.to_string()),
            market_selection_outcome: snapshot.market_selection_outcome.clone(),
            polymarket_condition_id: required_snapshot_string(
                snapshot.polymarket_condition_id.as_deref(),
                "polymarket_condition_id",
            )?,
            polymarket_market_slug: required_snapshot_string(
                snapshot.polymarket_market_slug.as_deref(),
                "polymarket_market_slug",
            )?,
            polymarket_question_id: required_snapshot_string(
                snapshot.polymarket_question_id.as_deref(),
                "polymarket_question_id",
            )?,
            up_instrument_id: required_snapshot_string(
                snapshot.up_instrument_id.as_deref(),
                "up_instrument_id",
            )?,
            down_instrument_id: required_snapshot_string(
                snapshot.down_instrument_id.as_deref(),
                "down_instrument_id",
            )?,
            selected_market_observed_timestamp_ms,
            polymarket_market_start_timestamp_ms,
            polymarket_market_end_timestamp_ms,
        };
        if !phase8_market_selection_source_matches_strategy(&raw, market_selection_source) {
            return Err(anyhow!(
                "phase8 strategy input evidence does not match market selection source evidence"
            ));
        }
        Ok(raw)
    }
}

fn required_snapshot_string(value: Option<&str>, field: &str) -> Result<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("phase8 strategy input evidence requires {field}"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phase8MarketSelectionRuntimeProvenance {
    decision_evidence_path: String,
    decision_evidence_sha256: String,
    instrument_source_path: String,
    instrument_source_sha256: String,
}

impl Phase8MarketSelectionRuntimeProvenance {
    pub fn new(
        decision_evidence_path: impl AsRef<str>,
        decision_evidence_sha256: impl AsRef<str>,
        instrument_source_path: impl AsRef<str>,
        instrument_source_sha256: impl AsRef<str>,
    ) -> Result<Self> {
        let decision_evidence_path = decision_evidence_path.as_ref().trim();
        if decision_evidence_path.is_empty() {
            return Err(anyhow!(
                "phase8 market selection source requires decision_evidence_path"
            ));
        }
        let decision_evidence_sha256 = decision_evidence_sha256.as_ref().trim();
        if !phase8_is_sha256_hex(decision_evidence_sha256) {
            return Err(anyhow!(
                "phase8 market selection source requires decision_evidence_sha256"
            ));
        }
        let instrument_source_path = instrument_source_path.as_ref().trim();
        if instrument_source_path.is_empty() {
            return Err(anyhow!(
                "phase8 market selection source requires instrument_source_path"
            ));
        }
        let instrument_source_sha256 = instrument_source_sha256.as_ref().trim();
        if !phase8_is_sha256_hex(instrument_source_sha256) {
            return Err(anyhow!(
                "phase8 market selection source requires instrument_source_sha256"
            ));
        }
        Ok(Self {
            decision_evidence_path: decision_evidence_path.to_string(),
            decision_evidence_sha256: decision_evidence_sha256.to_string(),
            instrument_source_path: instrument_source_path.to_string(),
            instrument_source_sha256: instrument_source_sha256.to_string(),
        })
    }

    pub fn decision_evidence_path(&self) -> &str {
        &self.decision_evidence_path
    }

    pub fn decision_evidence_sha256(&self) -> &str {
        &self.decision_evidence_sha256
    }

    pub fn instrument_source_path(&self) -> &str {
        &self.instrument_source_path
    }

    pub fn instrument_source_sha256(&self) -> &str {
        &self.instrument_source_sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Phase8MarketSelectionSourceEvidenceFile {
    record_kind: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime_provenance: Option<Phase8MarketSelectionRuntimeProvenance>,
    market_selection_timestamp_ms: u64,
    candidate_market_start_timestamps_ms: Vec<u64>,
    market_selection_outcome: String,
    polymarket_condition_id: String,
    polymarket_market_slug: String,
    polymarket_question_id: String,
    up_instrument_id: String,
    down_instrument_id: String,
    selected_market_observed_timestamp_ms: u64,
    polymarket_market_start_timestamp_ms: u64,
    polymarket_market_end_timestamp_ms: u64,
}

impl Phase8MarketSelectionSourceEvidenceFile {
    pub fn candidate_market_start_timestamps_ms(&self) -> &[u64] {
        &self.candidate_market_start_timestamps_ms
    }

    pub fn runtime_provenance(&self) -> Option<&Phase8MarketSelectionRuntimeProvenance> {
        self.runtime_provenance.as_ref()
    }

    pub fn with_runtime_provenance(
        mut self,
        provenance: Phase8MarketSelectionRuntimeProvenance,
    ) -> Self {
        self.runtime_provenance = Some(provenance);
        self
    }

    pub fn from_market_family_selection(
        market_selection_timestamp_ms: u64,
        candidate_windows: &[MarketSelectionCandidateWindow],
        selected: &SelectedBinaryOptionMarket,
    ) -> Result<Self> {
        let selected_window = candidate_windows
            .iter()
            .find(|window| {
                window.outcome == selected.selection_outcome
                    && window.start_timestamp_milliseconds == selected.start_timestamp_milliseconds
            })
            .ok_or_else(|| {
                anyhow!(
                    "selected market start timestamp does not match configured candidate window"
                )
            })?;
        if selected.source_identity.market_slug != selected_window.market_slug {
            return Err(anyhow!(
                "selected market identity does not match configured candidate window"
            ));
        }
        let market_selection_outcome = match selected.selection_outcome {
            MarketSelectionOutcome::Current => PHASE8_MARKET_SELECTION_OUTCOME_CURRENT,
            MarketSelectionOutcome::Next => PHASE8_MARKET_SELECTION_OUTCOME_NEXT,
        };
        let market_end_ms = selected.expiration_timestamp_milliseconds;
        if market_end_ms <= market_selection_timestamp_ms {
            return Err(anyhow!(
                "selected market expiration timestamp must be after selection timestamp"
            ));
        }

        Ok(Self {
            record_kind: PHASE8_MARKET_SELECTION_SOURCE_RECORD_KIND.to_string(),
            source: PHASE8_MARKET_SELECTION_SOURCE.to_string(),
            runtime_provenance: None,
            market_selection_timestamp_ms,
            candidate_market_start_timestamps_ms: candidate_windows
                .iter()
                .map(|window| window.start_timestamp_milliseconds)
                .collect(),
            market_selection_outcome: market_selection_outcome.to_string(),
            polymarket_condition_id: selected.source_identity.condition_id.clone(),
            polymarket_market_slug: selected.source_identity.market_slug.clone(),
            polymarket_question_id: selected.source_identity.question_id.clone(),
            up_instrument_id: selected.up_instrument_id.to_string(),
            down_instrument_id: selected.down_instrument_id.to_string(),
            selected_market_observed_timestamp_ms: market_selection_timestamp_ms,
            polymarket_market_start_timestamp_ms: selected.start_timestamp_milliseconds,
            polymarket_market_end_timestamp_ms: market_end_ms,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8CanaryPreflight {
    pub head_sha: String,
    pub root_config_sha256: String,
    pub no_submit_report_status: Phase8CanaryPreflightStatus,
    pub strategy_input_audit_status: Phase8StrategyInputAuditStatus,
    pub max_live_order_count: Option<u32>,
    pub max_notional_per_order: Option<String>,
    pub block_reasons: Vec<Phase8CanaryBlockReason>,
}

impl Phase8CanaryPreflight {
    pub fn can_enter_live_runner(&self) -> bool {
        self.block_reasons.is_empty()
            && self.no_submit_report_status == Phase8CanaryPreflightStatus::AcceptedByGate
            && self.strategy_input_audit_status == Phase8StrategyInputAuditStatus::Approved
            && self.max_live_order_count == Some(PHASE8_REQUIRED_LIVE_ORDER_CAP)
    }
}

pub async fn evaluate_phase8_canary_preflight(
    loaded: &LoadedBoltV3Config,
    head_sha: &str,
    strategy_audit: Phase8StrategyInputSafetyAudit,
) -> Phase8CanaryPreflight {
    let live_canary = loaded.root.live_canary.as_ref();
    let mut block_reasons = strategy_audit.block_reasons.clone();
    let root_config_sha256 = match Phase8OperatorApprovalEnvelope::sha256_file(&loaded.root_path) {
        Ok(hash) => hash,
        Err(_) => {
            block_reasons.push(Phase8CanaryBlockReason::RootConfigHashUnavailable);
            String::new()
        }
    };

    let no_submit_report_status = match check_bolt_v3_live_canary_pre_consumption_gate(loaded).await
    {
        Ok(_) => Phase8CanaryPreflightStatus::AcceptedByGate,
        Err(BoltV3LiveCanaryGateError::ReadinessReportRead { .. }) => {
            block_reasons.push(Phase8CanaryBlockReason::MissingNoSubmitReadinessReport);
            Phase8CanaryPreflightStatus::Missing
        }
        Err(_) => {
            block_reasons.push(Phase8CanaryBlockReason::LiveCanaryGateRejected);
            Phase8CanaryPreflightStatus::RejectedByGate
        }
    };
    if !matches!(
        live_canary,
        Some(block) if block.max_live_order_count == PHASE8_REQUIRED_LIVE_ORDER_CAP
    ) {
        block_reasons.push(Phase8CanaryBlockReason::LiveOrderCountCapNotOne);
    }

    Phase8CanaryPreflight {
        head_sha: head_sha.trim().to_string(),
        root_config_sha256,
        no_submit_report_status,
        strategy_input_audit_status: strategy_audit.status,
        max_live_order_count: live_canary.map(|block| block.max_live_order_count),
        max_notional_per_order: live_canary.map(|block| block.max_notional_per_order.clone()),
        block_reasons,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8EvidenceRef {
    pub path_hash: String,
    pub record_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8SubmitAdmissionRef {
    pub status: String,
    pub admitted_order_count: u32,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8RuntimeCaptureRef {
    pub spool_root_hash: String,
    pub run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8NtLifecycleRef {
    pub kind: String,
    pub event_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase8CanaryOutcome {
    DryNoSubmitProof,
    BlockedBeforeSubmit,
    LiveCanaryProof,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8LiveOrderRef {
    pub strategy_instance_id_hash: String,
    pub client_order_id_hash: String,
    pub venue_order_id_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8LiveCanaryResultRefs {
    pub nt_submit_event_ref: Phase8EvidenceRef,
    pub venue_order_state_ref: Phase8EvidenceRef,
    pub strategy_cancel_ref: Option<Phase8EvidenceRef>,
    pub restart_reconciliation_ref: Phase8EvidenceRef,
    pub post_run_hygiene_ref: Phase8EvidenceRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Phase8CanaryEvidence {
    pub schema_version: u32,
    pub head_sha: String,
    pub root_config_sha256: String,
    pub ssm_manifest_sha256: String,
    pub ssm_manifest_ref: Phase8EvidenceRef,
    pub strategy_input_evidence_ref: Phase8EvidenceRef,
    #[serde(skip)]
    approved_strategy_instance_id_hash: String,
    pub approval_id_hash: String,
    pub max_live_order_count: u32,
    pub max_notional_per_order: String,
    pub decision_evidence_ref: Option<Phase8EvidenceRef>,
    pub submit_admission_ref: Phase8SubmitAdmissionRef,
    pub live_order_ref: Option<Phase8LiveOrderRef>,
    pub nt_submit_event_ref: Option<Phase8EvidenceRef>,
    pub venue_order_state_ref: Option<Phase8EvidenceRef>,
    pub strategy_cancel_ref: Option<Phase8EvidenceRef>,
    pub restart_reconciliation_ref: Option<Phase8EvidenceRef>,
    pub post_run_hygiene_ref: Option<Phase8EvidenceRef>,
    pub runtime_capture_ref: Phase8RuntimeCaptureRef,
    pub nt_lifecycle_refs: Vec<Phase8NtLifecycleRef>,
    pub outcome: Phase8CanaryOutcome,
    pub block_reasons: Vec<Phase8CanaryBlockReason>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8CanaryEvidenceInput {
    pub head_sha: String,
    pub root_config_sha256: String,
    pub ssm_manifest_sha256: String,
    pub ssm_manifest_ref: Phase8EvidenceRef,
    pub strategy_input_evidence_ref: Phase8EvidenceRef,
    pub approved_strategy_instance_id_hash: String,
    pub approval_id: String,
    pub max_live_order_count: u32,
    pub max_notional_per_order: Decimal,
    pub runtime_capture_ref: Phase8RuntimeCaptureRef,
}

impl Phase8CanaryEvidence {
    pub fn dry_no_submit_proof(
        input: Phase8CanaryEvidenceInput,
        decision_evidence_ref: Phase8EvidenceRef,
    ) -> Self {
        Self {
            schema_version: PHASE8_CANARY_EVIDENCE_SCHEMA_VERSION,
            head_sha: input.head_sha,
            root_config_sha256: input.root_config_sha256,
            ssm_manifest_sha256: input.ssm_manifest_sha256,
            ssm_manifest_ref: input.ssm_manifest_ref,
            strategy_input_evidence_ref: input.strategy_input_evidence_ref,
            approved_strategy_instance_id_hash: input.approved_strategy_instance_id_hash,
            approval_id_hash: sha256_text(&input.approval_id),
            max_live_order_count: input.max_live_order_count,
            max_notional_per_order: input.max_notional_per_order.to_string(),
            decision_evidence_ref: Some(decision_evidence_ref),
            submit_admission_ref: Phase8SubmitAdmissionRef {
                status: SUBMIT_ADMISSION_STATUS_REJECTED.to_string(),
                admitted_order_count: 0,
                reason: BLOCKED_BEFORE_LIVE_ORDER_REASON.to_string(),
            },
            live_order_ref: None,
            nt_submit_event_ref: None,
            venue_order_state_ref: None,
            strategy_cancel_ref: None,
            restart_reconciliation_ref: None,
            post_run_hygiene_ref: None,
            runtime_capture_ref: input.runtime_capture_ref,
            nt_lifecycle_refs: Vec::new(),
            outcome: Phase8CanaryOutcome::DryNoSubmitProof,
            block_reasons: vec![Phase8CanaryBlockReason::BlockedBeforeLiveOrder],
        }
    }

    pub fn blocked_before_submit(
        input: Phase8CanaryEvidenceInput,
        block_reasons: Vec<Phase8CanaryBlockReason>,
    ) -> Self {
        Self {
            schema_version: PHASE8_CANARY_EVIDENCE_SCHEMA_VERSION,
            head_sha: input.head_sha,
            root_config_sha256: input.root_config_sha256,
            ssm_manifest_sha256: input.ssm_manifest_sha256,
            ssm_manifest_ref: input.ssm_manifest_ref,
            strategy_input_evidence_ref: input.strategy_input_evidence_ref,
            approved_strategy_instance_id_hash: input.approved_strategy_instance_id_hash,
            approval_id_hash: sha256_text(&input.approval_id),
            max_live_order_count: input.max_live_order_count,
            max_notional_per_order: input.max_notional_per_order.to_string(),
            decision_evidence_ref: None,
            submit_admission_ref: Phase8SubmitAdmissionRef {
                status: SUBMIT_ADMISSION_STATUS_REJECTED.to_string(),
                admitted_order_count: 0,
                reason: BLOCKED_BEFORE_SUBMIT_REASON.to_string(),
            },
            live_order_ref: None,
            nt_submit_event_ref: None,
            venue_order_state_ref: None,
            strategy_cancel_ref: None,
            restart_reconciliation_ref: None,
            post_run_hygiene_ref: None,
            runtime_capture_ref: input.runtime_capture_ref,
            nt_lifecycle_refs: Vec::new(),
            outcome: Phase8CanaryOutcome::BlockedBeforeSubmit,
            block_reasons,
        }
    }

    pub fn live_canary_proof(
        input: Phase8CanaryEvidenceInput,
        decision_evidence_ref: Phase8EvidenceRef,
        live_order_ref: Phase8LiveOrderRef,
        result_refs: Phase8LiveCanaryResultRefs,
        admitted_order_count: u32,
    ) -> Result<Self> {
        validate_phase8_canary_input(&input)?;
        validate_phase8_live_admitted_order_count(
            input.max_live_order_count,
            admitted_order_count,
        )?;
        validate_phase8_evidence_ref(stringify!(decision_evidence_ref), &decision_evidence_ref)?;
        validate_phase8_live_order_ref(&live_order_ref)?;
        if live_order_ref.strategy_instance_id_hash != input.approved_strategy_instance_id_hash {
            return Err(anyhow!(
                "phase8 live canary proof live_order_ref.strategy_instance_id_hash does not match approved financial envelope"
            ));
        }
        validate_phase8_evidence_ref(
            stringify!(nt_submit_event_ref),
            &result_refs.nt_submit_event_ref,
        )?;
        validate_phase8_evidence_ref(
            stringify!(venue_order_state_ref),
            &result_refs.venue_order_state_ref,
        )?;
        if let Some(strategy_cancel_ref) = &result_refs.strategy_cancel_ref {
            validate_phase8_evidence_ref(stringify!(strategy_cancel_ref), strategy_cancel_ref)?;
        }
        validate_phase8_evidence_ref(
            stringify!(restart_reconciliation_ref),
            &result_refs.restart_reconciliation_ref,
        )?;
        validate_phase8_evidence_ref(
            stringify!(post_run_hygiene_ref),
            &result_refs.post_run_hygiene_ref,
        )?;
        Ok(Self {
            schema_version: PHASE8_CANARY_EVIDENCE_SCHEMA_VERSION,
            head_sha: input.head_sha,
            root_config_sha256: input.root_config_sha256,
            ssm_manifest_sha256: input.ssm_manifest_sha256,
            ssm_manifest_ref: input.ssm_manifest_ref,
            strategy_input_evidence_ref: input.strategy_input_evidence_ref,
            approved_strategy_instance_id_hash: input.approved_strategy_instance_id_hash,
            approval_id_hash: sha256_text(&input.approval_id),
            max_live_order_count: input.max_live_order_count,
            max_notional_per_order: input.max_notional_per_order.to_string(),
            decision_evidence_ref: Some(decision_evidence_ref),
            submit_admission_ref: Phase8SubmitAdmissionRef {
                status: SUBMIT_ADMISSION_STATUS_ACCEPTED.to_string(),
                admitted_order_count,
                reason: NT_ADAPTER_SUBMIT_PROVEN_REASON.to_string(),
            },
            live_order_ref: Some(live_order_ref),
            nt_submit_event_ref: Some(result_refs.nt_submit_event_ref),
            venue_order_state_ref: Some(result_refs.venue_order_state_ref),
            strategy_cancel_ref: result_refs.strategy_cancel_ref,
            restart_reconciliation_ref: Some(result_refs.restart_reconciliation_ref),
            post_run_hygiene_ref: Some(result_refs.post_run_hygiene_ref),
            runtime_capture_ref: input.runtime_capture_ref,
            nt_lifecycle_refs: Vec::new(),
            outcome: Phase8CanaryOutcome::LiveCanaryProof,
            block_reasons: Vec::new(),
        })
    }

    pub fn write_json_file(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate_before_write()?;
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| {
                anyhow!(
                    "failed to create phase8 canary evidence directory `{}`: {source}",
                    parent.display()
                )
            })?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|source| anyhow!("failed to serialize phase8 canary evidence: {source}"))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|source| match source.kind() {
                std::io::ErrorKind::AlreadyExists => anyhow!(
                    "phase8 canary evidence `{}` already exists; refusing to overwrite",
                    path.display()
                ),
                _ => anyhow!(
                    "failed to create phase8 canary evidence `{}`: {source}",
                    path.display()
                ),
            })?;
        if let Err(source) = file.write_all(&bytes) {
            let _ = fs::remove_file(path);
            return Err(anyhow!(
                "failed to write phase8 canary evidence `{}`: {source}",
                path.display()
            ));
        }
        if let Err(source) = file.sync_all() {
            let _ = fs::remove_file(path);
            return Err(anyhow!(
                "failed to sync phase8 canary evidence `{}`: {source}",
                path.display()
            ));
        }
        Ok(())
    }

    fn validate_before_write(&self) -> Result<()> {
        validate_phase8_canary_identity_fields(
            self.schema_version,
            &self.head_sha,
            &self.approval_id_hash,
        )?;
        validate_phase8_sha256_field(
            stringify!(approved_strategy_instance_id_hash),
            &self.approved_strategy_instance_id_hash,
        )?;
        validate_phase8_canary_cap_values(self.max_live_order_count, &self.max_notional_per_order)?;
        validate_phase8_evidence_hashes(
            &self.root_config_sha256,
            &self.ssm_manifest_sha256,
            &self.ssm_manifest_ref,
            &self.strategy_input_evidence_ref,
            &self.runtime_capture_ref,
        )?;
        validate_phase8_nt_lifecycle_refs(&self.nt_lifecycle_refs)?;
        match self.outcome {
            Phase8CanaryOutcome::DryNoSubmitProof => {
                validate_phase8_live_refs_absent(self)?;
                validate_phase8_block_reasons_exact(
                    &self.block_reasons,
                    Phase8CanaryBlockReason::BlockedBeforeLiveOrder,
                )?;
                validate_phase8_submit_admission_ref(
                    &self.submit_admission_ref,
                    SUBMIT_ADMISSION_STATUS_REJECTED,
                    u32::MIN,
                    u32::MIN,
                    BLOCKED_BEFORE_LIVE_ORDER_REASON,
                )?;
                let decision_evidence_ref =
                    self.decision_evidence_ref.as_ref().ok_or_else(|| {
                        anyhow!(
                            "phase8 canary evidence {} must be present for dry proof",
                            stringify!(decision_evidence_ref)
                        )
                    })?;
                validate_phase8_evidence_ref(
                    stringify!(decision_evidence_ref),
                    decision_evidence_ref,
                )
            }
            Phase8CanaryOutcome::BlockedBeforeSubmit => {
                validate_phase8_optional_absent(
                    stringify!(decision_evidence_ref),
                    self.decision_evidence_ref.is_some(),
                )?;
                validate_phase8_live_refs_absent(self)?;
                validate_phase8_block_reasons_present(&self.block_reasons)?;
                validate_phase8_submit_admission_ref(
                    &self.submit_admission_ref,
                    SUBMIT_ADMISSION_STATUS_REJECTED,
                    u32::MIN,
                    u32::MIN,
                    BLOCKED_BEFORE_SUBMIT_REASON,
                )
            }
            Phase8CanaryOutcome::LiveCanaryProof => {
                validate_phase8_block_reasons_absent(&self.block_reasons)?;
                let required_admitted_order_count =
                    phase8_max_admitted_order_count(PHASE8_REQUIRED_LIVE_ORDER_CAP);
                validate_phase8_submit_admission_ref(
                    &self.submit_admission_ref,
                    SUBMIT_ADMISSION_STATUS_ACCEPTED,
                    required_admitted_order_count,
                    required_admitted_order_count,
                    NT_ADAPTER_SUBMIT_PROVEN_REASON,
                )?;
                let decision_evidence_ref =
                    self.decision_evidence_ref.as_ref().ok_or_else(|| {
                        anyhow!(
                            "phase8 canary evidence {} must be present for live proof",
                            stringify!(decision_evidence_ref)
                        )
                    })?;
                let live_order_ref = self.live_order_ref.as_ref().ok_or_else(|| {
                    anyhow!(
                        "phase8 canary evidence {} must be present for live proof",
                        stringify!(live_order_ref)
                    )
                })?;
                let nt_submit_event_ref = self.nt_submit_event_ref.as_ref().ok_or_else(|| {
                    anyhow!(
                        "phase8 canary evidence {} must be present for live proof",
                        stringify!(nt_submit_event_ref)
                    )
                })?;
                let venue_order_state_ref =
                    self.venue_order_state_ref.as_ref().ok_or_else(|| {
                        anyhow!(
                            "phase8 canary evidence {} must be present for live proof",
                            stringify!(venue_order_state_ref)
                        )
                    })?;
                let restart_reconciliation_ref =
                    self.restart_reconciliation_ref.as_ref().ok_or_else(|| {
                        anyhow!(
                            "phase8 canary evidence {} must be present for live proof",
                            stringify!(restart_reconciliation_ref)
                        )
                    })?;
                let post_run_hygiene_ref = self.post_run_hygiene_ref.as_ref().ok_or_else(|| {
                    anyhow!(
                        "phase8 canary evidence {} must be present for live proof",
                        stringify!(post_run_hygiene_ref)
                    )
                })?;
                validate_phase8_evidence_ref(
                    stringify!(decision_evidence_ref),
                    decision_evidence_ref,
                )?;
                validate_phase8_live_order_ref(live_order_ref)?;
                if live_order_ref.strategy_instance_id_hash
                    != self.approved_strategy_instance_id_hash
                {
                    return Err(anyhow!(
                        "phase8 live canary proof live_order_ref.strategy_instance_id_hash does not match approved financial envelope"
                    ));
                }
                validate_phase8_evidence_ref(stringify!(nt_submit_event_ref), nt_submit_event_ref)?;
                validate_phase8_evidence_ref(
                    stringify!(venue_order_state_ref),
                    venue_order_state_ref,
                )?;
                if let Some(strategy_cancel_ref) = &self.strategy_cancel_ref {
                    validate_phase8_evidence_ref(
                        stringify!(strategy_cancel_ref),
                        strategy_cancel_ref,
                    )?;
                }
                validate_phase8_evidence_ref(
                    stringify!(restart_reconciliation_ref),
                    restart_reconciliation_ref,
                )?;
                validate_phase8_evidence_ref(stringify!(post_run_hygiene_ref), post_run_hygiene_ref)
            }
        }
    }
}

fn validate_phase8_canary_identity_fields(
    schema_version: u32,
    head_sha: &str,
    approval_id_hash: &str,
) -> Result<()> {
    if schema_version != PHASE8_CANARY_EVIDENCE_SCHEMA_VERSION {
        return Err(anyhow!(
            "phase8 canary evidence {} expected {PHASE8_CANARY_EVIDENCE_SCHEMA_VERSION} got {schema_version}",
            stringify!(schema_version)
        ));
    }
    if head_sha.trim().is_empty() {
        return Err(anyhow!(
            "phase8 canary evidence {} must not be empty",
            stringify!(head_sha)
        ));
    }
    validate_phase8_sha256_field(stringify!(approval_id_hash), approval_id_hash)
}

fn validate_phase8_canary_cap_values(
    max_live_order_count: u32,
    max_notional_per_order: &str,
) -> Result<()> {
    if max_live_order_count != PHASE8_REQUIRED_LIVE_ORDER_CAP {
        return Err(anyhow!(
            "phase8 canary evidence {} expected {PHASE8_REQUIRED_LIVE_ORDER_CAP} got {max_live_order_count}",
            stringify!(max_live_order_count)
        ));
    }
    let max_notional_per_order = max_notional_per_order
        .parse::<Decimal>()
        .map_err(|source| {
            anyhow!(
                "phase8 canary evidence {} must be a decimal: {source}",
                stringify!(max_notional_per_order)
            )
        })?;
    if max_notional_per_order <= Decimal::ZERO {
        return Err(anyhow!(
            "phase8 canary evidence {} must be positive",
            stringify!(max_notional_per_order)
        ));
    }
    Ok(())
}

fn validate_phase8_block_reasons_exact(
    block_reasons: &[Phase8CanaryBlockReason],
    expected: Phase8CanaryBlockReason,
) -> Result<()> {
    if block_reasons == [expected] {
        Ok(())
    } else {
        Err(anyhow!(
            "phase8 canary evidence {} does not match expected outcome reason",
            stringify!(block_reasons)
        ))
    }
}

fn validate_phase8_block_reasons_absent(block_reasons: &[Phase8CanaryBlockReason]) -> Result<()> {
    if block_reasons.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "phase8 canary evidence {} must be empty for live proof",
            stringify!(block_reasons)
        ))
    }
}

fn validate_phase8_block_reasons_present(block_reasons: &[Phase8CanaryBlockReason]) -> Result<()> {
    if block_reasons.is_empty() {
        Err(anyhow!(
            "phase8 canary evidence {} must not be empty for blocked proof",
            stringify!(block_reasons)
        ))
    } else {
        Ok(())
    }
}

fn validate_phase8_live_refs_absent(evidence: &Phase8CanaryEvidence) -> Result<()> {
    validate_phase8_optional_absent(
        stringify!(live_order_ref),
        evidence.live_order_ref.is_some(),
    )?;
    validate_phase8_optional_absent(
        stringify!(nt_submit_event_ref),
        evidence.nt_submit_event_ref.is_some(),
    )?;
    validate_phase8_optional_absent(
        stringify!(venue_order_state_ref),
        evidence.venue_order_state_ref.is_some(),
    )?;
    validate_phase8_optional_absent(
        stringify!(strategy_cancel_ref),
        evidence.strategy_cancel_ref.is_some(),
    )?;
    validate_phase8_optional_absent(
        stringify!(restart_reconciliation_ref),
        evidence.restart_reconciliation_ref.is_some(),
    )?;
    validate_phase8_optional_absent(
        stringify!(post_run_hygiene_ref),
        evidence.post_run_hygiene_ref.is_some(),
    )
}

fn validate_phase8_optional_absent(field: &'static str, present: bool) -> Result<()> {
    if present {
        Err(anyhow!(
            "phase8 canary evidence {field} must be absent for non-live proof"
        ))
    } else {
        Ok(())
    }
}

fn validate_phase8_submit_admission_ref(
    submit_admission_ref: &Phase8SubmitAdmissionRef,
    expected_status: &str,
    min_admitted_order_count: u32,
    max_admitted_order_count: u32,
    expected_reason: &str,
) -> Result<()> {
    if submit_admission_ref.status != expected_status {
        return Err(anyhow!(
            "phase8 canary evidence {}.{} expected `{}` got `{}`",
            stringify!(submit_admission_ref),
            stringify!(status),
            expected_status,
            submit_admission_ref.status
        ));
    }
    if submit_admission_ref.admitted_order_count < min_admitted_order_count
        || submit_admission_ref.admitted_order_count > max_admitted_order_count
    {
        return Err(anyhow!(
            "phase8 canary evidence {}.{} expected {}..={} got {}",
            stringify!(submit_admission_ref),
            stringify!(admitted_order_count),
            min_admitted_order_count,
            max_admitted_order_count,
            submit_admission_ref.admitted_order_count
        ));
    }
    if submit_admission_ref.reason != expected_reason {
        return Err(anyhow!(
            "phase8 canary evidence {}.{} expected `{}` got `{}`",
            stringify!(submit_admission_ref),
            stringify!(reason),
            expected_reason,
            submit_admission_ref.reason
        ));
    }
    Ok(())
}

fn phase8_max_admitted_order_count(max_live_order_count: u32) -> u32 {
    max_live_order_count.saturating_mul(2)
}

fn validate_phase8_live_admitted_order_count(
    max_live_order_count: u32,
    admitted_order_count: u32,
) -> Result<()> {
    let required_admitted_order_count = phase8_max_admitted_order_count(max_live_order_count);
    if admitted_order_count != required_admitted_order_count {
        return Err(anyhow!(
            "phase8 live canary proof admitted_order_count expected {} got {}",
            required_admitted_order_count,
            admitted_order_count
        ));
    }
    Ok(())
}

fn validate_phase8_canary_input(input: &Phase8CanaryEvidenceInput) -> Result<()> {
    validate_phase8_evidence_hashes(
        &input.root_config_sha256,
        &input.ssm_manifest_sha256,
        &input.ssm_manifest_ref,
        &input.strategy_input_evidence_ref,
        &input.runtime_capture_ref,
    )?;
    validate_phase8_sha256_field(
        stringify!(approved_strategy_instance_id_hash),
        &input.approved_strategy_instance_id_hash,
    )?;
    if input.max_live_order_count != PHASE8_REQUIRED_LIVE_ORDER_CAP {
        return Err(anyhow!(
            "phase8 live canary proof {} expected {PHASE8_REQUIRED_LIVE_ORDER_CAP} got {}",
            stringify!(max_live_order_count),
            input.max_live_order_count
        ));
    }
    if input.max_notional_per_order <= Decimal::ZERO {
        return Err(anyhow!(
            "phase8 live canary proof {} must be positive",
            stringify!(max_notional_per_order)
        ));
    }
    Ok(())
}

fn validate_phase8_evidence_hashes(
    root_config_sha256: &str,
    ssm_manifest_sha256: &str,
    ssm_manifest_ref: &Phase8EvidenceRef,
    strategy_input_evidence_ref: &Phase8EvidenceRef,
    runtime_capture_ref: &Phase8RuntimeCaptureRef,
) -> Result<()> {
    validate_phase8_sha256_field(stringify!(root_config_sha256), root_config_sha256)?;
    validate_phase8_sha256_field(stringify!(ssm_manifest_sha256), ssm_manifest_sha256)?;
    validate_phase8_evidence_ref(stringify!(ssm_manifest_ref), ssm_manifest_ref)?;
    validate_phase8_evidence_ref(
        stringify!(strategy_input_evidence_ref),
        strategy_input_evidence_ref,
    )?;
    validate_phase8_nested_sha256_field(
        stringify!(runtime_capture_ref),
        stringify!(spool_root_hash),
        &runtime_capture_ref.spool_root_hash,
    )?;
    validate_phase8_required_text_field(
        stringify!(runtime_capture_ref.run_id),
        &runtime_capture_ref.run_id,
    )
}

fn validate_phase8_evidence_ref(
    label: &'static str,
    evidence_ref: &Phase8EvidenceRef,
) -> Result<()> {
    validate_phase8_nested_sha256_field(label, stringify!(path_hash), &evidence_ref.path_hash)?;
    validate_phase8_nested_sha256_field(label, stringify!(record_hash), &evidence_ref.record_hash)
}

fn validate_phase8_live_order_ref(live_order_ref: &Phase8LiveOrderRef) -> Result<()> {
    validate_phase8_nested_sha256_field(
        stringify!(live_order_ref),
        stringify!(strategy_instance_id_hash),
        &live_order_ref.strategy_instance_id_hash,
    )?;
    validate_phase8_nested_sha256_field(
        stringify!(live_order_ref),
        stringify!(client_order_id_hash),
        &live_order_ref.client_order_id_hash,
    )?;
    validate_phase8_nested_sha256_field(
        stringify!(live_order_ref),
        stringify!(venue_order_id_hash),
        &live_order_ref.venue_order_id_hash,
    )
}

fn validate_phase8_nt_lifecycle_refs(nt_lifecycle_refs: &[Phase8NtLifecycleRef]) -> Result<()> {
    for nt_lifecycle_ref in nt_lifecycle_refs {
        validate_phase8_required_text_field(
            stringify!(nt_lifecycle_refs.kind),
            &nt_lifecycle_ref.kind,
        )?;
        validate_phase8_sha256_field(
            stringify!(nt_lifecycle_refs.event_hash),
            &nt_lifecycle_ref.event_hash,
        )?;
    }
    Ok(())
}

fn validate_phase8_required_text_field(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(anyhow!(
            "phase8 live canary proof {field} must not be empty"
        ))
    } else {
        Ok(())
    }
}

fn validate_phase8_nested_sha256_field(parent: &str, child: &str, value: &str) -> Result<()> {
    let mut field = String::from(parent);
    field.push('.');
    field.push_str(child);
    validate_phase8_sha256_field(&field, value)
}

fn validate_phase8_sha256_field(field: &str, value: &str) -> Result<()> {
    if phase8_is_sha256_hex(value) {
        Ok(())
    } else {
        Err(anyhow!(
            "phase8 live canary proof {field} must be a sha256 hash"
        ))
    }
}

fn phase8_is_sha256_hex(value: &str) -> bool {
    let digest = Sha256::digest([]);
    value.len() == digest.len() + digest.len()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn phase8_reject_parent_dir(path: &str, label: &str) -> Result<()> {
    if Path::new(path)
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(anyhow!(
            "phase8 {label} path must not contain parent directory traversal"
        ));
    }
    Ok(())
}

/// Spent-nonce evidence written over the operator nonce file on consume. It carries no
/// hardcoded record-kind string — its only job is to overwrite the nonce with content that no
/// longer hashes to the approved `approval_nonce_sha256` (the serde field names are the keys;
/// both values are runtime data). Nothing reads this record back.
#[derive(serde::Serialize)]
struct Phase8ApprovalNonceSpentEvidence<'a> {
    spent_unix_secs: i64,
    consumed_approval_nonce_sha256: &'a str,
}

/// Filesystem suffix for the sibling temp file used to atomically overwrite the nonce. This is an
/// internal staging detail, not a runtime config value: the spent record is written to this temp
/// file, fsynced, then renamed over the nonce so the spend is crash-atomic and durable.
const PHASE8_NONCE_SPEND_TEMP_SUFFIX: &str = ".spending";

/// Spend the one-shot operator-approval nonce by overwriting the nonce evidence file once
/// the approval is consumed (A1/A2). After this, `validate_approval_nonce` fails for that
/// approval because the on-disk nonce no longer hashes to the approved
/// `approval_nonce_sha256`, so even a deliberately-deleted consumption marker cannot re-arm
/// the one-time approval. Re-approval requires the operator to mint a fresh nonce. This is
/// the one point where bolt-v3 WRITES operator evidence — by design, a nonce is single-use.
///
/// The overwrite is DURABLE and ATOMIC: the spent record is staged in a sibling temp file and
/// fsynced, then atomically renamed over the nonce, and the parent directory is fsynced. Once
/// this returns `Ok` the spent record is on stable storage, so a crash immediately afterwards
/// (in particular, after this but before the consumption marker is written) still leaves the
/// pre-consumption gate failing closed on the spent nonce. A plain in-place rewrite is not
/// crash-safe — a power loss could lose the rewrite (reverting the nonce to the approved value)
/// or leave a torn file — which is why the spend must complete durably before the caller proceeds
/// to write the consumption marker.
fn spend_phase8_approval_nonce(
    nonce_path: &str,
    approved_nonce_sha256: &str,
    spent_unix_secs: i64,
) -> Result<()> {
    let spent = Phase8ApprovalNonceSpentEvidence {
        spent_unix_secs,
        consumed_approval_nonce_sha256: approved_nonce_sha256,
    };
    let bytes = serde_json::to_vec_pretty(&spent)
        .map_err(|source| anyhow!("failed to serialize phase8 spent-nonce record: {source}"))?;
    let nonce_file = Path::new(nonce_path);
    let mut temp_os = nonce_file.as_os_str().to_owned();
    temp_os.push(PHASE8_NONCE_SPEND_TEMP_SUFFIX);
    let temp_path = PathBuf::from(temp_os);
    {
        let mut temp_file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp_path)
            .map_err(|source| {
                anyhow!(
                    "failed to open phase8 spent-nonce temp `{}`: {source}",
                    temp_path.display()
                )
            })?;
        if let Err(source) = temp_file.write_all(&bytes) {
            let _ = fs::remove_file(&temp_path);
            return Err(anyhow!(
                "failed to write phase8 spent-nonce temp `{}`: {source}",
                temp_path.display()
            ));
        }
        if let Err(source) = temp_file.sync_all() {
            let _ = fs::remove_file(&temp_path);
            return Err(anyhow!(
                "failed to sync phase8 spent-nonce temp `{}`: {source}",
                temp_path.display()
            ));
        }
    }
    if let Err(source) = fs::rename(&temp_path, nonce_file) {
        let _ = fs::remove_file(&temp_path);
        return Err(anyhow!(
            "failed to spend phase8 operator approval nonce `{nonce_path}`: {source}"
        ));
    }
    if let Some(parent) = nonce_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        let dir = fs::File::open(parent).map_err(|source| {
            anyhow!(
                "failed to open phase8 nonce directory `{}` for durability sync: {source}",
                parent.display()
            )
        })?;
        dir.sync_all().map_err(|source| {
            anyhow!(
                "failed to sync phase8 nonce directory `{}`: {source}",
                parent.display()
            )
        })?;
    }
    Ok(())
}

fn phase8_resolve_configured_path(root_path: &Path, configured: &str) -> PathBuf {
    let path = PathBuf::from(configured.trim());
    if path.is_absolute() {
        return path;
    }
    match root_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        Some(parent) => parent.join(path),
        None => path,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phase8OperatorApprovalEnvelope {
    pub head_sha: String,
    pub root_toml_path: String,
    pub root_toml_sha256: String,
    pub approval_envelope_sha256: String,
    pub ssm_manifest_path: String,
    pub ssm_manifest_sha256: String,
    pub strategy_input_evidence_path: String,
    pub strategy_input_evidence_sha256: String,
    pub financial_envelope_path: String,
    pub financial_envelope_sha256: String,
    pub pre_run_state_path: String,
    pub pre_run_state_sha256: String,
    pub abort_plan_path: String,
    pub abort_plan_sha256: String,
    pub operator_approval_id: String,
    pub approval_not_before_unix_secs: i64,
    pub approval_not_after_unix_secs: i64,
    pub approval_nonce_path: String,
    pub approval_nonce_sha256: String,
    pub approval_consumption_path: String,
    pub canary_evidence_path: String,
    pub strategy_cancel_path: Option<String>,
}

impl Phase8OperatorApprovalEnvelope {
    pub fn from_env() -> Result<Self> {
        let root_toml_path = required_path_env("BOLT_V3_PHASE8_ROOT_TOML_PATH")?;
        let loaded = load_bolt_v3_config(Path::new(&root_toml_path))?;
        let operator_evidence = loaded
            .root
            .live_canary
            .as_ref()
            .and_then(|block| block.operator_evidence.as_ref())
            .ok_or_else(|| {
                anyhow!(
                    "phase8 operator approval requires `[live_canary].operator_evidence` in root TOML"
                )
            })?;
        Ok(Self {
            head_sha: required_env("BOLT_V3_PHASE8_HEAD_SHA")?,
            root_toml_sha256: Self::sha256_file(&root_toml_path)?,
            approval_envelope_sha256: operator_evidence.approval_envelope_sha256.clone(),
            root_toml_path,
            ssm_manifest_path: required_path_env("BOLT_V3_PHASE8_SSM_MANIFEST_PATH")?,
            ssm_manifest_sha256: required_env("BOLT_V3_PHASE8_SSM_MANIFEST_SHA256")?,
            strategy_input_evidence_path: required_path_env(
                "BOLT_V3_PHASE8_STRATEGY_INPUT_EVIDENCE_PATH",
            )?,
            strategy_input_evidence_sha256: required_env(
                "BOLT_V3_PHASE8_STRATEGY_INPUT_EVIDENCE_SHA256",
            )?,
            financial_envelope_path: required_path_env("BOLT_V3_PHASE8_FINANCIAL_ENVELOPE_PATH")?,
            financial_envelope_sha256: required_env("BOLT_V3_PHASE8_FINANCIAL_ENVELOPE_SHA256")?,
            pre_run_state_path: required_path_env("BOLT_V3_PHASE8_PRE_RUN_STATE_PATH")?,
            pre_run_state_sha256: required_env("BOLT_V3_PHASE8_PRE_RUN_STATE_SHA256")?,
            abort_plan_path: required_path_env("BOLT_V3_PHASE8_ABORT_PLAN_PATH")?,
            abort_plan_sha256: required_env("BOLT_V3_PHASE8_ABORT_PLAN_SHA256")?,
            operator_approval_id: required_env("BOLT_V3_PHASE8_OPERATOR_APPROVAL_ID")?,
            approval_not_before_unix_secs: required_i64_env(
                "BOLT_V3_PHASE8_APPROVAL_NOT_BEFORE_UNIX_SECONDS",
            )?,
            approval_not_after_unix_secs: required_i64_env(
                "BOLT_V3_PHASE8_APPROVAL_NOT_AFTER_UNIX_SECONDS",
            )?,
            approval_nonce_path: required_path_env("BOLT_V3_PHASE8_APPROVAL_NONCE_PATH")?,
            approval_nonce_sha256: required_env("BOLT_V3_PHASE8_APPROVAL_NONCE_SHA256")?,
            approval_consumption_path: required_path_env(
                "BOLT_V3_PHASE8_APPROVAL_CONSUMPTION_PATH",
            )?,
            canary_evidence_path: required_path_env("BOLT_V3_PHASE8_EVIDENCE_PATH")?,
            strategy_cancel_path: optional_path_env("BOLT_V3_PHASE8_STRATEGY_CANCEL_PATH")?,
        })
    }

    pub fn validate_against(
        &self,
        current_head_sha: &str,
        current_root_toml_sha256: &str,
        live_canary_approval_id: &str,
    ) -> Result<()> {
        if self.head_sha != current_head_sha {
            return Err(anyhow!(
                "phase8 operator approval head_sha does not match current head"
            ));
        }
        if self.root_toml_sha256 != current_root_toml_sha256 {
            return Err(anyhow!(
                "phase8 operator approval root_toml_sha256 does not match current root TOML"
            ));
        }
        let current_ssm_manifest_sha256 = Self::sha256_file(&self.ssm_manifest_path)?;
        if self.ssm_manifest_sha256 != current_ssm_manifest_sha256 {
            return Err(anyhow!(
                "phase8 operator approval ssm_manifest_sha256 does not match current SSM manifest"
            ));
        }
        let current_strategy_input_evidence_sha256 =
            Self::sha256_file(&self.strategy_input_evidence_path)?;
        if self.strategy_input_evidence_sha256 != current_strategy_input_evidence_sha256 {
            return Err(anyhow!(
                "phase8 operator approval strategy_input_evidence_sha256 does not match current strategy input evidence"
            ));
        }
        if self.operator_approval_id != live_canary_approval_id {
            return Err(anyhow!(
                "phase8 operator approval id does not match `[live_canary]`"
            ));
        }
        Ok(())
    }

    pub fn validate_and_consume_against(
        &self,
        current_head_sha: &str,
        current_root_toml_sha256: &str,
        live_canary_approval_id: &str,
        loaded: &LoadedBoltV3Config,
        current_unix_secs: i64,
    ) -> Result<()> {
        self.validate_approved_evidence_against(
            current_head_sha,
            current_root_toml_sha256,
            live_canary_approval_id,
            loaded,
            current_unix_secs,
        )?;
        self.consume_approval_after_live_runner_entry_validation(loaded, current_unix_secs)
    }

    pub fn validate_approved_evidence_against(
        &self,
        current_head_sha: &str,
        current_root_toml_sha256: &str,
        live_canary_approval_id: &str,
        loaded: &LoadedBoltV3Config,
        current_unix_secs: i64,
    ) -> Result<()> {
        self.validate_against(
            current_head_sha,
            current_root_toml_sha256,
            live_canary_approval_id,
        )?;
        let approval_consumption_path = self.approval_consumption_path_against(loaded)?;
        self.validate_approval_not_consumed(&approval_consumption_path)?;
        self.validate_approval_window(current_unix_secs)?;
        self.validate_financial_envelope_against(loaded)?;
        self.validate_pre_run_state_against(loaded)?;
        self.validate_abort_plan_against(loaded)?;
        self.canary_evidence_path_against(loaded)?;
        self.validate_strategy_cancel_path_against(loaded)?;
        self.validate_approval_nonce()
    }

    pub fn consume_approval_after_live_runner_entry_validation(
        &self,
        loaded: &LoadedBoltV3Config,
        current_unix_secs: i64,
    ) -> Result<()> {
        let approval_consumption_path = self.approval_consumption_path_against(loaded)?;
        self.validate_approval_not_consumed(&approval_consumption_path)?;
        self.validate_approval_window(current_unix_secs)?;
        self.validate_approval_nonce()?;
        let canary_evidence_path = self.canary_evidence_path_against(loaded)?;
        let strategy_cancel_path = self.validate_strategy_cancel_path_against(loaded)?;
        // Spend the one-shot nonce FIRST — before the consumption marker — so the spend is the
        // FIRST durable one-way state transition on consume. `spend_phase8_approval_nonce` returns
        // only after the spent record is durable (fsync + atomic rename + parent-dir fsync), so any
        // crash after this point leaves the pre-consumption gate failing closed on the spent nonce
        // (`validate_approval_nonce` no longer matches the approved `approval_nonce_sha256`),
        // regardless of the marker. Writing the marker first (the previous order) left a crash
        // window where the marker was durable but the nonce was still un-spent: a later deletion of
        // that marker — by an operator inside the window or by host tmp cleanup — could re-arm the
        // approval. Spending first closes that window: the marker is written only once the nonce is
        // already, durably, single-use, so the marker's existence implies the nonce is spent (A1/A2).
        spend_phase8_approval_nonce(
            &self.approval_nonce_path,
            &self.approval_nonce_sha256,
            current_unix_secs,
        )?;
        self.write_approval_consumption_evidence(
            current_unix_secs,
            canary_evidence_path,
            strategy_cancel_path,
            &approval_consumption_path,
        )
    }

    fn validate_financial_envelope_against(&self, loaded: &LoadedBoltV3Config) -> Result<()> {
        let approved = self.read_financial_envelope()?;
        let loaded = Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(
            loaded,
            &approved.strategy_instance_id,
        )?;
        approved.validate_matches(&loaded)
    }

    fn validate_pre_run_state_against(&self, loaded: &LoadedBoltV3Config) -> Result<()> {
        let path = Path::new(&self.pre_run_state_path);
        let approved: Phase8PreRunStateEvidenceFile = Self::read_json_file_with_expected_sha256(
            path,
            &self.pre_run_state_sha256,
            "phase8 pre-run state evidence",
            "phase8 operator approval pre_run_state_sha256 does not match current pre-run state evidence",
        )?;
        let approved_financial = self.read_financial_envelope()?;
        let loaded = Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(
            loaded,
            &approved_financial.strategy_instance_id,
        )?;
        approved.validate_matches_loaded(&loaded)
    }

    fn validate_abort_plan_against(&self, loaded: &LoadedBoltV3Config) -> Result<()> {
        let path = Path::new(&self.abort_plan_path);
        let approved: Phase8AbortPlanEvidenceFile = Self::read_json_file_with_expected_sha256(
            path,
            &self.abort_plan_sha256,
            "phase8 abort plan evidence",
            "phase8 operator approval abort_plan_sha256 does not match current abort plan evidence",
        )?;
        let approved_financial = self.read_financial_envelope()?;
        let loaded = Phase8FinancialEnvelopeEvidenceFile::from_loaded_for_strategy(
            loaded,
            &approved_financial.strategy_instance_id,
        )?;
        approved.validate_collector_derived_matches_loaded(&loaded)
    }

    fn validate_strategy_cancel_path_against<'a>(
        &self,
        loaded: &'a LoadedBoltV3Config,
    ) -> Result<Option<&'a str>> {
        let configured = loaded
            .root
            .live_canary
            .as_ref()
            .and_then(|block| block.operator_evidence.as_ref())
            .and_then(|evidence| evidence.strategy_cancel_path.as_deref());
        match (self.strategy_cancel_path.as_deref(), configured) {
            (None, None) => Ok(None),
            (Some(approved), Some(configured)) if approved == configured => Ok(Some(configured)),
            (None, Some(_)) => Err(anyhow!(
                "phase8 operator approval strategy_cancel_path missing but `[live_canary].operator_evidence.strategy_cancel_path` is configured"
            )),
            (Some(_), None) => Err(anyhow!(
                "phase8 operator approval strategy_cancel_path is set but `[live_canary].operator_evidence.strategy_cancel_path` is not configured"
            )),
            (Some(_), Some(_)) => Err(anyhow!(
                "phase8 operator approval strategy_cancel_path does not match `[live_canary].operator_evidence.strategy_cancel_path`"
            )),
        }
    }

    fn approval_consumption_path_against(&self, loaded: &LoadedBoltV3Config) -> Result<PathBuf> {
        let configured = loaded
            .root
            .live_canary
            .as_ref()
            .and_then(|block| block.operator_evidence.as_ref())
            .map(|evidence| evidence.approval_consumption_path.as_str())
            .ok_or_else(|| {
                anyhow!(
                    "phase8 operator approval approval_consumption_path cannot be validated because `[live_canary].operator_evidence.approval_consumption_path` is not configured"
                )
            })?;
        phase8_reject_parent_dir(
            &self.approval_consumption_path,
            "operator approval approval_consumption_path",
        )?;
        phase8_reject_parent_dir(
            configured,
            "`[live_canary].operator_evidence.approval_consumption_path`",
        )?;
        let approved_path =
            phase8_resolve_configured_path(&loaded.root_path, &self.approval_consumption_path);
        let configured_path = phase8_resolve_configured_path(&loaded.root_path, configured);
        if approved_path != configured_path {
            return Err(anyhow!(
                "phase8 operator approval approval_consumption_path does not match `[live_canary].operator_evidence.approval_consumption_path`"
            ));
        }
        Ok(configured_path)
    }

    fn canary_evidence_path_against<'a>(&self, loaded: &'a LoadedBoltV3Config) -> Result<&'a str> {
        let configured = loaded
            .root
            .live_canary
            .as_ref()
            .and_then(|block| block.operator_evidence.as_ref())
            .map(|evidence| evidence.canary_evidence_path.as_str())
            .ok_or_else(|| {
                anyhow!(
                    "phase8 operator approval canary_evidence_path cannot be validated because `[live_canary].operator_evidence.canary_evidence_path` is not configured"
                )
            })?;
        phase8_reject_parent_dir(
            &self.canary_evidence_path,
            "operator approval canary_evidence_path",
        )?;
        phase8_reject_parent_dir(
            configured,
            "`[live_canary].operator_evidence.canary_evidence_path`",
        )?;
        let approved_path =
            phase8_resolve_configured_path(&loaded.root_path, &self.canary_evidence_path);
        let configured_path = phase8_resolve_configured_path(&loaded.root_path, configured);
        if approved_path != configured_path {
            return Err(anyhow!(
                "phase8 operator approval canary_evidence_path does not match `[live_canary].operator_evidence.canary_evidence_path`"
            ));
        }
        Ok(configured)
    }

    fn validate_approval_not_consumed(&self, path: &Path) -> Result<()> {
        if path.try_exists().map_err(|source| {
            anyhow!(
                "failed to inspect phase8 operator approval consumption `{}`: {source}",
                path.display()
            )
        })? {
            return Err(self.approval_already_consumed_error(path));
        }
        Ok(())
    }

    fn read_financial_envelope(&self) -> Result<Phase8FinancialEnvelopeEvidenceFile> {
        let path = Path::new(&self.financial_envelope_path);
        Self::read_json_file_with_expected_sha256(
            path,
            &self.financial_envelope_sha256,
            "phase8 financial envelope",
            "phase8 operator approval financial_envelope_sha256 does not match current financial envelope",
        )
    }

    pub fn approved_strategy_instance_id_hash(&self) -> Result<String> {
        Ok(sha256_text(
            &self.read_financial_envelope()?.strategy_instance_id,
        ))
    }

    pub fn approved_price_to_beat_source(&self) -> Result<String> {
        Ok(self.read_financial_envelope()?.price_to_beat_source)
    }

    fn validate_approval_window(&self, current_unix_secs: i64) -> Result<()> {
        if self.approval_not_after_unix_secs <= self.approval_not_before_unix_secs {
            return Err(anyhow!(
                "phase8 operator approval not_after must be greater than not_before"
            ));
        }
        if current_unix_secs < self.approval_not_before_unix_secs {
            return Err(anyhow!("phase8 operator approval is not yet valid"));
        }
        if current_unix_secs > self.approval_not_after_unix_secs {
            return Err(anyhow!("phase8 operator approval is expired"));
        }
        Ok(())
    }

    fn validate_approval_nonce(&self) -> Result<()> {
        let current_nonce_sha256 = Self::sha256_file(&self.approval_nonce_path)?;
        if self.approval_nonce_sha256 != current_nonce_sha256 {
            return Err(anyhow!(
                "phase8 operator approval nonce sha256 does not match current nonce evidence"
            ));
        }
        Ok(())
    }

    fn write_approval_consumption_evidence(
        &self,
        current_unix_secs: i64,
        canary_evidence_path: &str,
        strategy_cancel_path: Option<&str>,
        path: &Path,
    ) -> Result<()> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent).map_err(|source| {
                anyhow!(
                    "failed to create phase8 approval consumption evidence directory `{}`: {source}",
                    parent.display()
                )
            })?;
        }
        let evidence = Phase8ApprovalConsumptionEvidence {
            schema_version: APPROVAL_CONSUMPTION_SCHEMA_VERSION,
            record_kind: APPROVAL_CONSUMPTION_RECORD_KIND,
            head_sha: &self.head_sha,
            root_toml_sha256: &self.root_toml_sha256,
            approval_envelope_sha256: &self.approval_envelope_sha256,
            ssm_manifest_sha256: &self.ssm_manifest_sha256,
            strategy_input_evidence_sha256: &self.strategy_input_evidence_sha256,
            financial_envelope_sha256: &self.financial_envelope_sha256,
            pre_run_state_sha256: &self.pre_run_state_sha256,
            abort_plan_sha256: &self.abort_plan_sha256,
            approval_id_hash: sha256_text(&self.operator_approval_id),
            approval_nonce_sha256: &self.approval_nonce_sha256,
            approval_not_before_unix_secs: self.approval_not_before_unix_secs,
            approval_not_after_unix_secs: self.approval_not_after_unix_secs,
            canary_evidence_path_hash: sha256_text(canary_evidence_path),
            strategy_cancel_path_hash: strategy_cancel_path.map(sha256_text),
            consumed_unix_secs: current_unix_secs,
        };
        let bytes = serde_json::to_vec_pretty(&evidence).map_err(|source| {
            anyhow!("failed to serialize phase8 approval consumption evidence: {source}")
        })?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|source| match source.kind() {
                std::io::ErrorKind::AlreadyExists => self.approval_already_consumed_error(path),
                _ => anyhow!(
                    "failed to create phase8 operator approval consumption `{}`: {source}",
                    path.display()
                ),
            })?;
        if let Err(source) = file.write_all(&bytes) {
            let _ = fs::remove_file(path);
            return Err(anyhow!(
                "failed to write phase8 operator approval consumption `{}`: {source}",
                path.display()
            ));
        }
        if let Err(source) = file.sync_all() {
            let _ = fs::remove_file(path);
            return Err(anyhow!(
                "failed to sync phase8 operator approval consumption `{}`: {source}",
                path.display()
            ));
        }
        Ok(())
    }

    fn approval_already_consumed_error(&self, path: &Path) -> anyhow::Error {
        anyhow!(
            "phase8 operator approval consumption `{}` already consumed; refusing to replay",
            path.display()
        )
    }

    pub fn sha256_file(path: impl AsRef<Path>) -> Result<String> {
        let path = path.as_ref();
        let file = fs::File::open(path).map_err(|source| {
            anyhow!(
                "failed to open phase8 sha256 input `{}`: {source}",
                path.display()
            )
        })?;
        let mut reader = BufReader::new(file);
        let mut digest = Sha256::new();
        let mut buffer = [0; PHASE8_SHA256_BUFFER_BYTES];
        loop {
            let length = reader.read(&mut buffer).map_err(|source| {
                anyhow!(
                    "failed to read phase8 sha256 input `{}`: {source}",
                    path.display()
                )
            })?;
            if length == 0 {
                break;
            }
            digest.update(&buffer[..length]);
        }
        Ok(format!("{:x}", digest.finalize()))
    }

    fn read_json_file_with_expected_sha256<T>(
        path: impl AsRef<Path>,
        expected_sha256: &str,
        artifact_label: &'static str,
        mismatch_message: &'static str,
    ) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|source| {
            anyhow!(
                "failed to open {artifact_label} `{}`: {source}",
                path.display()
            )
        })?;
        let current_sha256 = Self::sha256_bytes(&bytes);
        if expected_sha256 != current_sha256 {
            return Err(anyhow!(mismatch_message));
        }
        serde_json::from_slice(&bytes).map_err(|source| {
            anyhow!(
                "failed to parse {artifact_label} `{}`: {source}",
                path.display()
            )
        })
    }

    fn sha256_bytes(bytes: &[u8]) -> String {
        let mut digest = Sha256::new();
        digest.update(bytes);
        format!("{:x}", digest.finalize())
    }

    pub fn root_path(&self) -> PathBuf {
        PathBuf::from(&self.root_toml_path)
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Phase8FinancialEnvelopeEvidenceFile {
    max_live_order_count: u32,
    max_notional_per_order: String,
    strategy_instance_id: String,
    oms_type: String,
    execution_client_id: String,
    configured_target_id: String,
    target_kind: String,
    rotating_market_family: String,
    underlying_asset: String,
    cadence_secs: i64,
    cadence_slug_token: String,
    market_selection_rule: String,
    retry_interval_secs: i64,
    blocked_after_secs: i64,
    price_to_beat_source: String,
    edge_threshold_basis_points: i64,
    order_notional_target: String,
    maximum_position_notional: String,
    book_impact_cap_bps: i64,
    entry_side: String,
    entry_position_side: String,
    entry_order_type: String,
    entry_time_in_force: String,
    entry_expire_time_unix_nanos: Option<i64>,
    entry_trigger_price: Option<f64>,
    entry_activation_price: Option<f64>,
    entry_trigger_type: Option<String>,
    entry_trigger_instrument_id: Option<String>,
    entry_trailing_offset: Option<f64>,
    entry_trailing_offset_type: Option<String>,
    entry_is_post_only: bool,
    entry_is_reduce_only: bool,
    entry_is_quote_quantity: bool,
    exit_side: String,
    exit_position_side: String,
    exit_order_type: String,
    exit_time_in_force: String,
    exit_expire_time_unix_nanos: Option<i64>,
    exit_trigger_price: Option<f64>,
    exit_activation_price: Option<f64>,
    exit_trigger_type: Option<String>,
    exit_trigger_instrument_id: Option<String>,
    exit_trailing_offset: Option<f64>,
    exit_trailing_offset_type: Option<String>,
    exit_is_post_only: bool,
    exit_is_reduce_only: bool,
    exit_is_quote_quantity: bool,
    forced_exit_side: String,
    forced_exit_position_side: String,
    forced_exit_order_type: String,
    forced_exit_time_in_force: String,
    forced_exit_expire_time_unix_nanos: Option<i64>,
    forced_exit_trigger_price: Option<f64>,
    forced_exit_activation_price: Option<f64>,
    forced_exit_trigger_type: Option<String>,
    forced_exit_trigger_instrument_id: Option<String>,
    forced_exit_trailing_offset: Option<f64>,
    forced_exit_trailing_offset_type: Option<String>,
    forced_exit_is_post_only: bool,
    forced_exit_is_reduce_only: bool,
    forced_exit_is_quote_quantity: bool,
}

impl Phase8FinancialEnvelopeEvidenceFile {
    pub fn max_notional_per_order(&self) -> &str {
        &self.max_notional_per_order
    }

    pub fn execution_client_id(&self) -> &str {
        &self.execution_client_id
    }

    pub fn strategy_instance_id(&self) -> &str {
        &self.strategy_instance_id
    }

    pub fn configured_target_id(&self) -> &str {
        &self.configured_target_id
    }

    pub fn price_to_beat_source(&self) -> &str {
        &self.price_to_beat_source
    }

    pub fn from_loaded_for_strategy(
        loaded: &LoadedBoltV3Config,
        strategy_instance_id: &str,
    ) -> Result<Self> {
        let live_canary = loaded
            .root
            .live_canary
            .as_ref()
            .ok_or_else(|| anyhow!("phase8 financial envelope requires `[live_canary]`"))?;
        if strategy_instance_id.trim().is_empty() {
            return Err(anyhow!(
                "phase8 financial envelope requires non-empty strategy_instance_id"
            ));
        }
        let mut matching_strategies = loaded.strategies.iter().filter(|strategy| {
            strategy.config.strategy_instance_id.as_str() == strategy_instance_id
        });
        let strategy = matching_strategies.next().ok_or_else(|| {
            anyhow!(
                "phase8 financial envelope strategy_instance_id does not match a loaded strategy"
            )
        })?;
        if matching_strategies.next().is_some() {
            return Err(anyhow!(
                "phase8 financial envelope strategy_instance_id matches multiple loaded strategies"
            ));
        }
        let strategy = &strategy.config;
        let target = strategy.target.as_table().ok_or_else(|| {
            anyhow!("phase8 financial envelope strategy target must be a TOML table")
        })?;
        let parameters = strategy.parameters.as_table().ok_or_else(|| {
            anyhow!("phase8 financial envelope strategy parameters must be a TOML table")
        })?;
        let runtime_parameters = parameters
            .get(stringify!(runtime))
            .and_then(toml::Value::as_table)
            .ok_or_else(|| {
                anyhow!(
                    "phase8 financial envelope strategy runtime parameters must be a TOML table"
                )
            })?;
        let entry_order = parameters
            .get(stringify!(entry_order))
            .and_then(toml::Value::as_table)
            .ok_or_else(|| {
                anyhow!("phase8 financial envelope strategy entry order must be a TOML table")
            })?;
        let exit_order = parameters
            .get(stringify!(exit_order))
            .and_then(toml::Value::as_table)
            .ok_or_else(|| {
                anyhow!("phase8 financial envelope strategy exit order must be a TOML table")
            })?;
        let forced_exit_order = parameters
            .get(stringify!(forced_exit_order))
            .and_then(toml::Value::as_table)
            .ok_or_else(|| {
                anyhow!("phase8 financial envelope strategy forced exit order must be a TOML table")
            })?;
        let price_to_beat_source = price_to_beat_source_from_target(&strategy.target)?;
        Ok(Self {
            max_live_order_count: live_canary.max_live_order_count,
            max_notional_per_order: live_canary.max_notional_per_order.clone(),
            strategy_instance_id: strategy.strategy_instance_id.clone(),
            oms_type: nt_enum_variant_lowercase(strategy.oms_type),
            execution_client_id: strategy.execution_client_id.to_string(),
            configured_target_id: required_toml_string(target, stringify!(configured_target_id))?,
            target_kind: required_toml_string(target, stringify!(kind))?,
            rotating_market_family: required_toml_string(
                target,
                stringify!(rotating_market_family),
            )?,
            underlying_asset: required_toml_string(target, stringify!(underlying_asset))?,
            cadence_secs: required_toml_integer(target, stringify!(cadence_secs))?,
            cadence_slug_token: required_toml_string(target, stringify!(cadence_slug_token))?,
            market_selection_rule: required_toml_string(target, stringify!(market_selection_rule))?,
            retry_interval_secs: required_toml_integer(target, stringify!(retry_interval_secs))?,
            blocked_after_secs: required_toml_integer(target, stringify!(blocked_after_secs))?,
            price_to_beat_source,
            edge_threshold_basis_points: required_toml_integer(
                parameters,
                stringify!(edge_threshold_basis_points),
            )?,
            order_notional_target: required_toml_string(
                parameters,
                stringify!(order_notional_target),
            )?,
            maximum_position_notional: required_toml_string(
                parameters,
                stringify!(maximum_position_notional),
            )?,
            book_impact_cap_bps: required_toml_integer(
                runtime_parameters,
                stringify!(book_impact_cap_bps),
            )?,
            entry_side: required_toml_nt_enum::<OrderSide>(
                entry_order,
                stringify!(side),
                stringify!(OrderSide),
            )?,
            entry_position_side: required_toml_nt_enum::<PositionSide>(
                entry_order,
                stringify!(position_side),
                stringify!(PositionSide),
            )?,
            entry_order_type: required_toml_nt_enum::<OrderType>(
                entry_order,
                stringify!(order_type),
                stringify!(OrderType),
            )?,
            entry_time_in_force: required_toml_nt_enum::<TimeInForce>(
                entry_order,
                stringify!(time_in_force),
                stringify!(TimeInForce),
            )?,
            entry_expire_time_unix_nanos: optional_toml_integer(
                entry_order,
                stringify!(expire_time_unix_nanos),
            )?,
            entry_trigger_price: optional_toml_float(entry_order, stringify!(trigger_price))?,
            entry_activation_price: optional_toml_float(entry_order, stringify!(activation_price))?,
            entry_trigger_type: optional_toml_nt_enum::<TriggerType>(
                entry_order,
                stringify!(trigger_type),
                stringify!(TriggerType),
            )?,
            entry_trigger_instrument_id: optional_toml_string(
                entry_order,
                stringify!(trigger_instrument_id),
            )?,
            entry_trailing_offset: optional_toml_float(entry_order, stringify!(trailing_offset))?,
            entry_trailing_offset_type: optional_toml_nt_enum::<TrailingOffsetType>(
                entry_order,
                stringify!(trailing_offset_type),
                stringify!(TrailingOffsetType),
            )?,
            entry_is_post_only: required_toml_bool(entry_order, stringify!(is_post_only))?,
            entry_is_reduce_only: required_toml_bool(entry_order, stringify!(is_reduce_only))?,
            entry_is_quote_quantity: required_toml_bool(
                entry_order,
                stringify!(is_quote_quantity),
            )?,
            exit_side: required_toml_nt_enum::<OrderSide>(
                exit_order,
                stringify!(side),
                stringify!(OrderSide),
            )?,
            exit_position_side: required_toml_nt_enum::<PositionSide>(
                exit_order,
                stringify!(position_side),
                stringify!(PositionSide),
            )?,
            exit_order_type: required_toml_nt_enum::<OrderType>(
                exit_order,
                stringify!(order_type),
                stringify!(OrderType),
            )?,
            exit_time_in_force: required_toml_nt_enum::<TimeInForce>(
                exit_order,
                stringify!(time_in_force),
                stringify!(TimeInForce),
            )?,
            exit_expire_time_unix_nanos: optional_toml_integer(
                exit_order,
                stringify!(expire_time_unix_nanos),
            )?,
            exit_trigger_price: optional_toml_float(exit_order, stringify!(trigger_price))?,
            exit_activation_price: optional_toml_float(exit_order, stringify!(activation_price))?,
            exit_trigger_type: optional_toml_nt_enum::<TriggerType>(
                exit_order,
                stringify!(trigger_type),
                stringify!(TriggerType),
            )?,
            exit_trigger_instrument_id: optional_toml_string(
                exit_order,
                stringify!(trigger_instrument_id),
            )?,
            exit_trailing_offset: optional_toml_float(exit_order, stringify!(trailing_offset))?,
            exit_trailing_offset_type: optional_toml_nt_enum::<TrailingOffsetType>(
                exit_order,
                stringify!(trailing_offset_type),
                stringify!(TrailingOffsetType),
            )?,
            exit_is_post_only: required_toml_bool(exit_order, stringify!(is_post_only))?,
            exit_is_reduce_only: required_toml_bool(exit_order, stringify!(is_reduce_only))?,
            exit_is_quote_quantity: required_toml_bool(exit_order, stringify!(is_quote_quantity))?,
            forced_exit_side: required_toml_nt_enum::<OrderSide>(
                forced_exit_order,
                stringify!(side),
                stringify!(OrderSide),
            )?,
            forced_exit_position_side: required_toml_nt_enum::<PositionSide>(
                forced_exit_order,
                stringify!(position_side),
                stringify!(PositionSide),
            )?,
            forced_exit_order_type: required_toml_nt_enum::<OrderType>(
                forced_exit_order,
                stringify!(order_type),
                stringify!(OrderType),
            )?,
            forced_exit_time_in_force: required_toml_nt_enum::<TimeInForce>(
                forced_exit_order,
                stringify!(time_in_force),
                stringify!(TimeInForce),
            )?,
            forced_exit_expire_time_unix_nanos: optional_toml_integer(
                forced_exit_order,
                stringify!(expire_time_unix_nanos),
            )?,
            forced_exit_trigger_price: optional_toml_float(
                forced_exit_order,
                stringify!(trigger_price),
            )?,
            forced_exit_activation_price: optional_toml_float(
                forced_exit_order,
                stringify!(activation_price),
            )?,
            forced_exit_trigger_type: optional_toml_nt_enum::<TriggerType>(
                forced_exit_order,
                stringify!(trigger_type),
                stringify!(TriggerType),
            )?,
            forced_exit_trigger_instrument_id: optional_toml_string(
                forced_exit_order,
                stringify!(trigger_instrument_id),
            )?,
            forced_exit_trailing_offset: optional_toml_float(
                forced_exit_order,
                stringify!(trailing_offset),
            )?,
            forced_exit_trailing_offset_type: optional_toml_nt_enum::<TrailingOffsetType>(
                forced_exit_order,
                stringify!(trailing_offset_type),
                stringify!(TrailingOffsetType),
            )?,
            forced_exit_is_post_only: required_toml_bool(
                forced_exit_order,
                stringify!(is_post_only),
            )?,
            forced_exit_is_reduce_only: required_toml_bool(
                forced_exit_order,
                stringify!(is_reduce_only),
            )?,
            forced_exit_is_quote_quantity: required_toml_bool(
                forced_exit_order,
                stringify!(is_quote_quantity),
            )?,
        })
    }

    pub fn validate_matches(&self, loaded: &Self) -> Result<()> {
        if self.max_live_order_count != loaded.max_live_order_count {
            return Err(financial_envelope_mismatch(stringify!(
                max_live_order_count
            )));
        }
        if self.max_notional_per_order != loaded.max_notional_per_order {
            return Err(financial_envelope_mismatch(stringify!(
                max_notional_per_order
            )));
        }
        if self.strategy_instance_id != loaded.strategy_instance_id {
            return Err(financial_envelope_mismatch(stringify!(
                strategy_instance_id
            )));
        }
        if canonical_approved_oms_type(&self.oms_type)? != loaded.oms_type {
            return Err(financial_envelope_mismatch(stringify!(oms_type)));
        }
        if self.execution_client_id != loaded.execution_client_id {
            return Err(financial_envelope_mismatch(stringify!(execution_client_id)));
        }
        if self.configured_target_id != loaded.configured_target_id {
            return Err(financial_envelope_mismatch(stringify!(
                configured_target_id
            )));
        }
        if self.target_kind != loaded.target_kind {
            return Err(financial_envelope_mismatch(stringify!(target_kind)));
        }
        if self.rotating_market_family != loaded.rotating_market_family {
            return Err(financial_envelope_mismatch(stringify!(
                rotating_market_family
            )));
        }
        if self.underlying_asset != loaded.underlying_asset {
            return Err(financial_envelope_mismatch(stringify!(underlying_asset)));
        }
        if self.cadence_secs != loaded.cadence_secs {
            return Err(financial_envelope_mismatch(stringify!(cadence_secs)));
        }
        if self.cadence_slug_token != loaded.cadence_slug_token {
            return Err(financial_envelope_mismatch(stringify!(cadence_slug_token)));
        }
        if self.market_selection_rule != loaded.market_selection_rule {
            return Err(financial_envelope_mismatch(stringify!(
                market_selection_rule
            )));
        }
        if self.retry_interval_secs != loaded.retry_interval_secs {
            return Err(financial_envelope_mismatch(stringify!(retry_interval_secs)));
        }
        if self.blocked_after_secs != loaded.blocked_after_secs {
            return Err(financial_envelope_mismatch(stringify!(blocked_after_secs)));
        }
        if self.price_to_beat_source != loaded.price_to_beat_source {
            return Err(financial_envelope_mismatch(stringify!(
                price_to_beat_source
            )));
        }
        if self.edge_threshold_basis_points != loaded.edge_threshold_basis_points {
            return Err(financial_envelope_mismatch(stringify!(
                edge_threshold_basis_points
            )));
        }
        if self.order_notional_target != loaded.order_notional_target {
            return Err(financial_envelope_mismatch(stringify!(
                order_notional_target
            )));
        }
        if self.maximum_position_notional != loaded.maximum_position_notional {
            return Err(financial_envelope_mismatch(stringify!(
                maximum_position_notional
            )));
        }
        if self.book_impact_cap_bps != loaded.book_impact_cap_bps {
            return Err(financial_envelope_mismatch(stringify!(book_impact_cap_bps)));
        }
        if canonical_financial_envelope_nt_enum::<OrderSide>(
            &self.entry_side,
            stringify!(entry_side),
            stringify!(OrderSide),
        )? != loaded.entry_side
        {
            return Err(financial_envelope_mismatch(stringify!(entry_side)));
        }
        if canonical_financial_envelope_nt_enum::<PositionSide>(
            &self.entry_position_side,
            stringify!(entry_position_side),
            stringify!(PositionSide),
        )? != loaded.entry_position_side
        {
            return Err(financial_envelope_mismatch(stringify!(entry_position_side)));
        }
        if canonical_financial_envelope_nt_enum::<OrderType>(
            &self.entry_order_type,
            stringify!(entry_order_type),
            stringify!(OrderType),
        )? != loaded.entry_order_type
        {
            return Err(financial_envelope_mismatch(stringify!(entry_order_type)));
        }
        if canonical_financial_envelope_nt_enum::<TimeInForce>(
            &self.entry_time_in_force,
            stringify!(entry_time_in_force),
            stringify!(TimeInForce),
        )? != loaded.entry_time_in_force
        {
            return Err(financial_envelope_mismatch(stringify!(entry_time_in_force)));
        }
        if self.entry_expire_time_unix_nanos != loaded.entry_expire_time_unix_nanos {
            return Err(financial_envelope_mismatch(stringify!(
                entry_expire_time_unix_nanos
            )));
        }
        if self.entry_trigger_price != loaded.entry_trigger_price {
            return Err(financial_envelope_mismatch(stringify!(entry_trigger_price)));
        }
        if self.entry_activation_price != loaded.entry_activation_price {
            return Err(financial_envelope_mismatch(stringify!(
                entry_activation_price
            )));
        }
        if canonical_optional_financial_envelope_nt_enum::<TriggerType>(
            self.entry_trigger_type.as_deref(),
            stringify!(entry_trigger_type),
            stringify!(TriggerType),
        )? != loaded.entry_trigger_type
        {
            return Err(financial_envelope_mismatch(stringify!(entry_trigger_type)));
        }
        if self.entry_trigger_instrument_id != loaded.entry_trigger_instrument_id {
            return Err(financial_envelope_mismatch(stringify!(
                entry_trigger_instrument_id
            )));
        }
        if self.entry_trailing_offset != loaded.entry_trailing_offset {
            return Err(financial_envelope_mismatch(stringify!(
                entry_trailing_offset
            )));
        }
        if canonical_optional_financial_envelope_nt_enum::<TrailingOffsetType>(
            self.entry_trailing_offset_type.as_deref(),
            stringify!(entry_trailing_offset_type),
            stringify!(TrailingOffsetType),
        )? != loaded.entry_trailing_offset_type
        {
            return Err(financial_envelope_mismatch(stringify!(
                entry_trailing_offset_type
            )));
        }
        if self.entry_is_post_only != loaded.entry_is_post_only {
            return Err(financial_envelope_mismatch(stringify!(entry_is_post_only)));
        }
        if self.entry_is_reduce_only != loaded.entry_is_reduce_only {
            return Err(financial_envelope_mismatch(stringify!(
                entry_is_reduce_only
            )));
        }
        if self.entry_is_quote_quantity != loaded.entry_is_quote_quantity {
            return Err(financial_envelope_mismatch(stringify!(
                entry_is_quote_quantity
            )));
        }
        if canonical_financial_envelope_nt_enum::<OrderSide>(
            &self.exit_side,
            stringify!(exit_side),
            stringify!(OrderSide),
        )? != loaded.exit_side
        {
            return Err(financial_envelope_mismatch(stringify!(exit_side)));
        }
        if canonical_financial_envelope_nt_enum::<PositionSide>(
            &self.exit_position_side,
            stringify!(exit_position_side),
            stringify!(PositionSide),
        )? != loaded.exit_position_side
        {
            return Err(financial_envelope_mismatch(stringify!(exit_position_side)));
        }
        if canonical_financial_envelope_nt_enum::<OrderType>(
            &self.exit_order_type,
            stringify!(exit_order_type),
            stringify!(OrderType),
        )? != loaded.exit_order_type
        {
            return Err(financial_envelope_mismatch(stringify!(exit_order_type)));
        }
        if canonical_financial_envelope_nt_enum::<TimeInForce>(
            &self.exit_time_in_force,
            stringify!(exit_time_in_force),
            stringify!(TimeInForce),
        )? != loaded.exit_time_in_force
        {
            return Err(financial_envelope_mismatch(stringify!(exit_time_in_force)));
        }
        if self.exit_expire_time_unix_nanos != loaded.exit_expire_time_unix_nanos {
            return Err(financial_envelope_mismatch(stringify!(
                exit_expire_time_unix_nanos
            )));
        }
        if self.exit_trigger_price != loaded.exit_trigger_price {
            return Err(financial_envelope_mismatch(stringify!(exit_trigger_price)));
        }
        if self.exit_activation_price != loaded.exit_activation_price {
            return Err(financial_envelope_mismatch(stringify!(
                exit_activation_price
            )));
        }
        if canonical_optional_financial_envelope_nt_enum::<TriggerType>(
            self.exit_trigger_type.as_deref(),
            stringify!(exit_trigger_type),
            stringify!(TriggerType),
        )? != loaded.exit_trigger_type
        {
            return Err(financial_envelope_mismatch(stringify!(exit_trigger_type)));
        }
        if self.exit_trigger_instrument_id != loaded.exit_trigger_instrument_id {
            return Err(financial_envelope_mismatch(stringify!(
                exit_trigger_instrument_id
            )));
        }
        if self.exit_trailing_offset != loaded.exit_trailing_offset {
            return Err(financial_envelope_mismatch(stringify!(
                exit_trailing_offset
            )));
        }
        if canonical_optional_financial_envelope_nt_enum::<TrailingOffsetType>(
            self.exit_trailing_offset_type.as_deref(),
            stringify!(exit_trailing_offset_type),
            stringify!(TrailingOffsetType),
        )? != loaded.exit_trailing_offset_type
        {
            return Err(financial_envelope_mismatch(stringify!(
                exit_trailing_offset_type
            )));
        }
        if self.exit_is_post_only != loaded.exit_is_post_only {
            return Err(financial_envelope_mismatch(stringify!(exit_is_post_only)));
        }
        if self.exit_is_reduce_only != loaded.exit_is_reduce_only {
            return Err(financial_envelope_mismatch(stringify!(exit_is_reduce_only)));
        }
        if self.exit_is_quote_quantity != loaded.exit_is_quote_quantity {
            return Err(financial_envelope_mismatch(stringify!(
                exit_is_quote_quantity
            )));
        }
        if canonical_financial_envelope_nt_enum::<OrderSide>(
            &self.forced_exit_side,
            stringify!(forced_exit_side),
            stringify!(OrderSide),
        )? != loaded.forced_exit_side
        {
            return Err(financial_envelope_mismatch(stringify!(forced_exit_side)));
        }
        if canonical_financial_envelope_nt_enum::<PositionSide>(
            &self.forced_exit_position_side,
            stringify!(forced_exit_position_side),
            stringify!(PositionSide),
        )? != loaded.forced_exit_position_side
        {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_position_side
            )));
        }
        if canonical_financial_envelope_nt_enum::<OrderType>(
            &self.forced_exit_order_type,
            stringify!(forced_exit_order_type),
            stringify!(OrderType),
        )? != loaded.forced_exit_order_type
        {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_order_type
            )));
        }
        if canonical_financial_envelope_nt_enum::<TimeInForce>(
            &self.forced_exit_time_in_force,
            stringify!(forced_exit_time_in_force),
            stringify!(TimeInForce),
        )? != loaded.forced_exit_time_in_force
        {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_time_in_force
            )));
        }
        if self.forced_exit_expire_time_unix_nanos != loaded.forced_exit_expire_time_unix_nanos {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_expire_time_unix_nanos
            )));
        }
        if self.forced_exit_trigger_price != loaded.forced_exit_trigger_price {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_trigger_price
            )));
        }
        if self.forced_exit_activation_price != loaded.forced_exit_activation_price {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_activation_price
            )));
        }
        if canonical_optional_financial_envelope_nt_enum::<TriggerType>(
            self.forced_exit_trigger_type.as_deref(),
            stringify!(forced_exit_trigger_type),
            stringify!(TriggerType),
        )? != loaded.forced_exit_trigger_type
        {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_trigger_type
            )));
        }
        if self.forced_exit_trigger_instrument_id != loaded.forced_exit_trigger_instrument_id {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_trigger_instrument_id
            )));
        }
        if self.forced_exit_trailing_offset != loaded.forced_exit_trailing_offset {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_trailing_offset
            )));
        }
        if canonical_optional_financial_envelope_nt_enum::<TrailingOffsetType>(
            self.forced_exit_trailing_offset_type.as_deref(),
            stringify!(forced_exit_trailing_offset_type),
            stringify!(TrailingOffsetType),
        )? != loaded.forced_exit_trailing_offset_type
        {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_trailing_offset_type
            )));
        }
        if self.forced_exit_is_post_only != loaded.forced_exit_is_post_only {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_is_post_only
            )));
        }
        if self.forced_exit_is_reduce_only != loaded.forced_exit_is_reduce_only {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_is_reduce_only
            )));
        }
        if self.forced_exit_is_quote_quantity != loaded.forced_exit_is_quote_quantity {
            return Err(financial_envelope_mismatch(stringify!(
                forced_exit_is_quote_quantity
            )));
        }
        Ok(())
    }
}

fn financial_envelope_mismatch(field: &'static str) -> anyhow::Error {
    anyhow!("phase8 financial envelope `{field}` does not match loaded TOML")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Phase8PreRunStateSourceProofs<'a> {
    pub host_clock_skew_within_bound: bool,
    pub host_clock_skew_evidence_hash: &'a str,
    pub conflicting_open_orders_absent: bool,
    pub preexisting_position_absent: bool,
    pub venue_account_state_evidence_hash: &'a str,
    pub market_state_approved: bool,
    pub market_window_approved: bool,
    pub market_state_evidence_hash: &'a str,
    pub funding_margin_covers_max_notional_plus_fees: bool,
    pub funding_margin_evidence_hash: &'a str,
    pub single_runner_lock_acquired: bool,
    pub single_runner_lock_evidence_hash: &'a str,
    pub egress_identity_approved: bool,
    pub egress_identity_evidence_hash: &'a str,
    pub clob_v2_adapter_signing_verified: bool,
    pub clob_v2_adapter_signing_evidence_hash: &'a str,
    pub clob_v2_collateral_accounting_verified: bool,
    pub clob_v2_collateral_accounting_evidence_hash: &'a str,
    pub clob_v2_fee_behavior_verified: bool,
    pub clob_v2_fee_behavior_evidence_hash: &'a str,
    pub release_manifest_clob_signing_version: &'a str,
    pub release_manifest_nt_revision_matches_compiled_pin: bool,
    pub release_manifest_evidence_hash: &'a str,
}

fn canonical_approved_oms_type(value: &str) -> Result<String> {
    canonical_financial_envelope_nt_enum::<OmsType>(
        value,
        stringify!(oms_type),
        stringify!(OmsType),
    )
}

fn canonical_financial_envelope_nt_enum<T>(
    value: &str,
    field: &'static str,
    type_name: &'static str,
) -> Result<String>
where
    T: FromStr + Display,
{
    canonical_nt_enum::<T>(value).map_err(|_| {
        anyhow!("phase8 financial envelope `{field}` must be a NautilusTrader {type_name}")
    })
}

fn canonical_optional_financial_envelope_nt_enum<T>(
    value: Option<&str>,
    field: &'static str,
    type_name: &'static str,
) -> Result<Option<String>>
where
    T: FromStr + Display,
{
    value
        .map(|value| canonical_financial_envelope_nt_enum::<T>(value, field, type_name))
        .transpose()
}

fn canonical_loaded_toml_nt_enum<T>(
    value: &str,
    field: &'static str,
    type_name: &'static str,
) -> Result<String>
where
    T: FromStr + Display,
{
    canonical_nt_enum::<T>(value).map_err(|_| {
        anyhow!(
            "phase8 financial envelope loaded TOML field `{field}` must be a NautilusTrader {type_name}"
        )
    })
}

fn canonical_nt_enum<T>(value: &str) -> Result<String, T::Err>
where
    T: FromStr + Display,
{
    value.parse::<T>().map(nt_enum_variant_lowercase)
}

fn nt_enum_variant_lowercase(value: impl Display) -> String {
    value.to_string().to_ascii_lowercase()
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Phase8PreRunStateEvidenceFile {
    execution_client_id: String,
    configured_target_id: String,
    host_clock_skew_within_bound: bool,
    host_clock_skew_evidence_hash: String,
    conflicting_open_orders_absent: bool,
    preexisting_position_absent: bool,
    venue_account_state_evidence_hash: String,
    market_state_approved: bool,
    market_window_approved: bool,
    market_state_evidence_hash: String,
    funding_margin_covers_max_notional_plus_fees: bool,
    funding_margin_evidence_hash: String,
    single_runner_lock_acquired: bool,
    single_runner_lock_evidence_hash: String,
    egress_identity_approved: bool,
    egress_identity_evidence_hash: String,
    clob_v2_adapter_signing_verified: bool,
    clob_v2_adapter_signing_evidence_hash: String,
    clob_v2_collateral_accounting_verified: bool,
    clob_v2_collateral_accounting_evidence_hash: String,
    clob_v2_fee_behavior_verified: bool,
    clob_v2_fee_behavior_evidence_hash: String,
    release_manifest_clob_signing_version: String,
    release_manifest_nt_revision_matches_compiled_pin: bool,
    release_manifest_evidence_hash: String,
}

impl Phase8PreRunStateEvidenceFile {
    pub fn from_financial_envelope_and_source_proofs(
        loaded: &Phase8FinancialEnvelopeEvidenceFile,
        proofs: Phase8PreRunStateSourceProofs<'_>,
    ) -> Result<Self> {
        let artifact = Self {
            execution_client_id: loaded.execution_client_id.clone(),
            configured_target_id: loaded.configured_target_id.clone(),
            host_clock_skew_within_bound: proofs.host_clock_skew_within_bound,
            host_clock_skew_evidence_hash: proofs.host_clock_skew_evidence_hash.to_string(),
            conflicting_open_orders_absent: proofs.conflicting_open_orders_absent,
            preexisting_position_absent: proofs.preexisting_position_absent,
            venue_account_state_evidence_hash: proofs.venue_account_state_evidence_hash.to_string(),
            market_state_approved: proofs.market_state_approved,
            market_window_approved: proofs.market_window_approved,
            market_state_evidence_hash: proofs.market_state_evidence_hash.to_string(),
            funding_margin_covers_max_notional_plus_fees: proofs
                .funding_margin_covers_max_notional_plus_fees,
            funding_margin_evidence_hash: proofs.funding_margin_evidence_hash.to_string(),
            single_runner_lock_acquired: proofs.single_runner_lock_acquired,
            single_runner_lock_evidence_hash: proofs.single_runner_lock_evidence_hash.to_string(),
            egress_identity_approved: proofs.egress_identity_approved,
            egress_identity_evidence_hash: proofs.egress_identity_evidence_hash.to_string(),
            clob_v2_adapter_signing_verified: proofs.clob_v2_adapter_signing_verified,
            clob_v2_adapter_signing_evidence_hash: proofs
                .clob_v2_adapter_signing_evidence_hash
                .to_string(),
            clob_v2_collateral_accounting_verified: proofs.clob_v2_collateral_accounting_verified,
            clob_v2_collateral_accounting_evidence_hash: proofs
                .clob_v2_collateral_accounting_evidence_hash
                .to_string(),
            clob_v2_fee_behavior_verified: proofs.clob_v2_fee_behavior_verified,
            clob_v2_fee_behavior_evidence_hash: proofs
                .clob_v2_fee_behavior_evidence_hash
                .to_string(),
            release_manifest_clob_signing_version: proofs
                .release_manifest_clob_signing_version
                .to_string(),
            release_manifest_nt_revision_matches_compiled_pin: proofs
                .release_manifest_nt_revision_matches_compiled_pin,
            release_manifest_evidence_hash: proofs.release_manifest_evidence_hash.to_string(),
        };
        artifact.validate_matches_loaded(loaded)?;
        Ok(artifact)
    }

    pub fn validate_matches_loaded(
        &self,
        loaded: &Phase8FinancialEnvelopeEvidenceFile,
    ) -> Result<()> {
        if self.execution_client_id != loaded.execution_client_id {
            return Err(pre_run_state_mismatch(stringify!(execution_client_id)));
        }
        if self.configured_target_id != loaded.configured_target_id {
            return Err(pre_run_state_mismatch(stringify!(configured_target_id)));
        }
        require_pre_run_clearance(
            stringify!(host_clock_skew_within_bound),
            self.host_clock_skew_within_bound,
        )?;
        require_pre_run_sha256(
            stringify!(host_clock_skew_evidence_hash),
            &self.host_clock_skew_evidence_hash,
        )?;
        require_pre_run_clearance(
            stringify!(conflicting_open_orders_absent),
            self.conflicting_open_orders_absent,
        )?;
        require_pre_run_clearance(
            stringify!(preexisting_position_absent),
            self.preexisting_position_absent,
        )?;
        require_pre_run_sha256(
            stringify!(venue_account_state_evidence_hash),
            &self.venue_account_state_evidence_hash,
        )?;
        require_pre_run_clearance(
            stringify!(market_state_approved),
            self.market_state_approved,
        )?;
        require_pre_run_clearance(
            stringify!(market_window_approved),
            self.market_window_approved,
        )?;
        require_pre_run_sha256(
            stringify!(market_state_evidence_hash),
            &self.market_state_evidence_hash,
        )?;
        require_pre_run_clearance(
            stringify!(funding_margin_covers_max_notional_plus_fees),
            self.funding_margin_covers_max_notional_plus_fees,
        )?;
        require_pre_run_sha256(
            stringify!(funding_margin_evidence_hash),
            &self.funding_margin_evidence_hash,
        )?;
        require_pre_run_clearance(
            stringify!(single_runner_lock_acquired),
            self.single_runner_lock_acquired,
        )?;
        require_pre_run_sha256(
            stringify!(single_runner_lock_evidence_hash),
            &self.single_runner_lock_evidence_hash,
        )?;
        require_pre_run_clearance(
            stringify!(egress_identity_approved),
            self.egress_identity_approved,
        )?;
        require_pre_run_sha256(
            stringify!(egress_identity_evidence_hash),
            &self.egress_identity_evidence_hash,
        )?;
        require_pre_run_clearance(
            stringify!(clob_v2_adapter_signing_verified),
            self.clob_v2_adapter_signing_verified,
        )?;
        require_pre_run_sha256(
            stringify!(clob_v2_adapter_signing_evidence_hash),
            &self.clob_v2_adapter_signing_evidence_hash,
        )?;
        require_pre_run_clearance(
            stringify!(clob_v2_collateral_accounting_verified),
            self.clob_v2_collateral_accounting_verified,
        )?;
        require_pre_run_sha256(
            stringify!(clob_v2_collateral_accounting_evidence_hash),
            &self.clob_v2_collateral_accounting_evidence_hash,
        )?;
        require_pre_run_clearance(
            stringify!(clob_v2_fee_behavior_verified),
            self.clob_v2_fee_behavior_verified,
        )?;
        require_pre_run_sha256(
            stringify!(clob_v2_fee_behavior_evidence_hash),
            &self.clob_v2_fee_behavior_evidence_hash,
        )?;
        require_pre_run_string(
            stringify!(release_manifest_clob_signing_version),
            &self.release_manifest_clob_signing_version,
        )?;
        require_pre_run_clearance(
            stringify!(release_manifest_nt_revision_matches_compiled_pin),
            self.release_manifest_nt_revision_matches_compiled_pin,
        )?;
        require_pre_run_sha256(
            stringify!(release_manifest_evidence_hash),
            &self.release_manifest_evidence_hash,
        )
    }
}

fn require_pre_run_clearance(field: &'static str, satisfied: bool) -> Result<()> {
    if satisfied {
        Ok(())
    } else {
        Err(pre_run_state_blocked(field))
    }
}

fn require_pre_run_string(field: &'static str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(pre_run_state_blocked(field))
    } else {
        Ok(())
    }
}

fn require_pre_run_sha256(field: &'static str, value: &str) -> Result<()> {
    if phase8_is_sha256_hex(value) {
        Ok(())
    } else {
        Err(pre_run_state_blocked(field))
    }
}

fn pre_run_state_mismatch(field: &'static str) -> anyhow::Error {
    anyhow!("phase8 pre-run state `{field}` does not match loaded TOML")
}

fn pre_run_state_blocked(field: &'static str) -> anyhow::Error {
    anyhow!("phase8 pre-run state `{field}` is not satisfied")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Phase8AbortPlanSourceProofs<'a> {
    pub cancel_if_open_defined: bool,
    pub cancel_if_open_evidence_hash: &'a str,
    pub nt_accepted_venue_pending_abort_defined: bool,
    pub nt_accepted_venue_pending_abort_evidence_hash: &'a str,
    pub partial_fill_abort_defined: bool,
    pub partial_fill_abort_evidence_hash: &'a str,
    pub network_partition_during_submit_abort_defined: bool,
    pub network_partition_during_submit_abort_evidence_hash: &'a str,
    pub panic_gate_trip_abort_defined: bool,
    pub panic_gate_trip_abort_evidence_hash: &'a str,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Phase8AbortPlanEvidenceFile {
    execution_client_id: String,
    configured_target_id: String,
    source_collector_derived: bool,
    strategy_source_sha256: String,
    submit_admission_source_sha256: String,
    cancel_if_open_defined: bool,
    cancel_if_open_evidence_hash: String,
    nt_accepted_venue_pending_abort_defined: bool,
    nt_accepted_venue_pending_abort_evidence_hash: String,
    partial_fill_abort_defined: bool,
    partial_fill_abort_evidence_hash: String,
    network_partition_during_submit_abort_defined: bool,
    network_partition_during_submit_abort_evidence_hash: String,
    panic_gate_trip_abort_defined: bool,
    panic_gate_trip_abort_evidence_hash: String,
}

impl Phase8AbortPlanEvidenceFile {
    pub fn from_financial_envelope_and_source_proofs(
        loaded: &Phase8FinancialEnvelopeEvidenceFile,
        proofs: Phase8AbortPlanSourceProofs<'_>,
    ) -> Result<Self> {
        Self::from_financial_envelope_and_source_proofs_with_provenance(
            loaded, proofs, false, "", "",
        )
    }

    pub fn from_financial_envelope_and_collector_source_proofs(
        loaded: &Phase8FinancialEnvelopeEvidenceFile,
        proofs: Phase8AbortPlanSourceProofs<'_>,
        strategy_source_sha256: &str,
        submit_admission_source_sha256: &str,
    ) -> Result<Self> {
        Self::from_financial_envelope_and_source_proofs_with_provenance(
            loaded,
            proofs,
            true,
            strategy_source_sha256,
            submit_admission_source_sha256,
        )
    }

    fn from_financial_envelope_and_source_proofs_with_provenance(
        loaded: &Phase8FinancialEnvelopeEvidenceFile,
        proofs: Phase8AbortPlanSourceProofs<'_>,
        source_collector_derived: bool,
        strategy_source_sha256: &str,
        submit_admission_source_sha256: &str,
    ) -> Result<Self> {
        let artifact = Self {
            execution_client_id: loaded.execution_client_id.clone(),
            configured_target_id: loaded.configured_target_id.clone(),
            source_collector_derived,
            strategy_source_sha256: strategy_source_sha256.to_string(),
            submit_admission_source_sha256: submit_admission_source_sha256.to_string(),
            cancel_if_open_defined: proofs.cancel_if_open_defined,
            cancel_if_open_evidence_hash: proofs.cancel_if_open_evidence_hash.to_string(),
            nt_accepted_venue_pending_abort_defined: proofs.nt_accepted_venue_pending_abort_defined,
            nt_accepted_venue_pending_abort_evidence_hash: proofs
                .nt_accepted_venue_pending_abort_evidence_hash
                .to_string(),
            partial_fill_abort_defined: proofs.partial_fill_abort_defined,
            partial_fill_abort_evidence_hash: proofs.partial_fill_abort_evidence_hash.to_string(),
            network_partition_during_submit_abort_defined: proofs
                .network_partition_during_submit_abort_defined,
            network_partition_during_submit_abort_evidence_hash: proofs
                .network_partition_during_submit_abort_evidence_hash
                .to_string(),
            panic_gate_trip_abort_defined: proofs.panic_gate_trip_abort_defined,
            panic_gate_trip_abort_evidence_hash: proofs
                .panic_gate_trip_abort_evidence_hash
                .to_string(),
        };
        artifact.validate_matches_loaded(loaded)?;
        Ok(artifact)
    }

    pub fn validate_collector_derived_matches_loaded(
        &self,
        loaded: &Phase8FinancialEnvelopeEvidenceFile,
    ) -> Result<()> {
        self.validate_matches_loaded(loaded)?;
        if !self.source_collector_derived {
            return Err(abort_plan_blocked(stringify!(source_collector_derived)));
        }
        require_abort_plan_sha256(
            stringify!(strategy_source_sha256),
            &self.strategy_source_sha256,
        )?;
        require_abort_plan_sha256(
            stringify!(submit_admission_source_sha256),
            &self.submit_admission_source_sha256,
        )?;
        if self.strategy_source_sha256 != expected_abort_plan_strategy_source_sha256() {
            return Err(abort_plan_mismatch(stringify!(strategy_source_sha256)));
        }
        if self.submit_admission_source_sha256
            != expected_abort_plan_submit_admission_source_sha256()
        {
            return Err(abort_plan_mismatch(stringify!(
                submit_admission_source_sha256
            )));
        }
        Ok(())
    }

    pub fn validate_matches_loaded(
        &self,
        loaded: &Phase8FinancialEnvelopeEvidenceFile,
    ) -> Result<()> {
        if self.execution_client_id != loaded.execution_client_id {
            return Err(abort_plan_mismatch(stringify!(execution_client_id)));
        }
        if self.configured_target_id != loaded.configured_target_id {
            return Err(abort_plan_mismatch(stringify!(configured_target_id)));
        }
        require_abort_plan_path(
            stringify!(cancel_if_open_defined),
            self.cancel_if_open_defined,
        )?;
        require_abort_plan_sha256(
            stringify!(cancel_if_open_evidence_hash),
            &self.cancel_if_open_evidence_hash,
        )?;
        require_abort_plan_path(
            stringify!(nt_accepted_venue_pending_abort_defined),
            self.nt_accepted_venue_pending_abort_defined,
        )?;
        require_abort_plan_sha256(
            stringify!(nt_accepted_venue_pending_abort_evidence_hash),
            &self.nt_accepted_venue_pending_abort_evidence_hash,
        )?;
        require_abort_plan_path(
            stringify!(partial_fill_abort_defined),
            self.partial_fill_abort_defined,
        )?;
        require_abort_plan_sha256(
            stringify!(partial_fill_abort_evidence_hash),
            &self.partial_fill_abort_evidence_hash,
        )?;
        require_abort_plan_path(
            stringify!(network_partition_during_submit_abort_defined),
            self.network_partition_during_submit_abort_defined,
        )?;
        require_abort_plan_sha256(
            stringify!(network_partition_during_submit_abort_evidence_hash),
            &self.network_partition_during_submit_abort_evidence_hash,
        )?;
        require_abort_plan_path(
            stringify!(panic_gate_trip_abort_defined),
            self.panic_gate_trip_abort_defined,
        )?;
        require_abort_plan_sha256(
            stringify!(panic_gate_trip_abort_evidence_hash),
            &self.panic_gate_trip_abort_evidence_hash,
        )
    }
}

fn require_abort_plan_path(field: &'static str, defined: bool) -> Result<()> {
    if defined {
        Ok(())
    } else {
        Err(abort_plan_blocked(field))
    }
}

fn require_abort_plan_sha256(field: &'static str, value: &str) -> Result<()> {
    if phase8_is_sha256_hex(value) {
        Ok(())
    } else {
        Err(abort_plan_blocked(field))
    }
}

fn abort_plan_mismatch(field: &'static str) -> anyhow::Error {
    anyhow!("phase8 abort plan `{field}` does not match loaded TOML")
}

fn expected_abort_plan_strategy_source_sha256() -> String {
    // Hash the canonical bytes EMBEDDED IN THE BINARY at compile time
    // (`build.rs` re-emits them into `$OUT_DIR/strategy.canonical` via the same
    // walk the runtime digest uses) — layout-independent yet still hashing
    // compiled-in bytes, so tamper-evidence is preserved.
    sha256_text(include_str!(concat!(
        env!("OUT_DIR"),
        "/strategy.canonical"
    )))
}

fn expected_abort_plan_submit_admission_source_sha256() -> String {
    sha256_text(include_str!(concat!(
        env!("OUT_DIR"),
        "/submit_admission.canonical"
    )))
}

fn abort_plan_blocked(field: &'static str) -> anyhow::Error {
    anyhow!("phase8 abort plan `{field}` is not defined")
}

fn required_toml_string(
    table: &toml::map::Map<String, toml::Value>,
    field: &'static str,
) -> Result<String> {
    table
        .get(field)
        .and_then(toml::Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| anyhow!("phase8 financial envelope loaded TOML field `{field}` is missing"))
}

fn price_to_beat_source_from_target(target: &toml::Value) -> Result<String> {
    let subscription = target
        .as_table()
        .and_then(|target| target.get("gate_subscriptions"))
        .and_then(toml::Value::as_table)
        .and_then(|subscriptions| subscriptions.get(RESOLUTION_GATE_ROLE))
        .and_then(toml::Value::as_table)
        .ok_or_else(|| {
            anyhow!(
                "phase8 financial envelope requires target.gate_subscriptions.{RESOLUTION_GATE_ROLE}"
            )
        })?;
    let mapping = subscription
        .get("market_mappings")
        .and_then(toml::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(toml::Value::as_table)
        .next()
        .ok_or_else(|| {
            anyhow!("phase8 financial envelope requires a target gate market mapping")
        })?;
    let resolution_kind = mapping
        .get("resolution_kind")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow!("phase8 financial envelope target gate resolution_kind is missing")
        })?;
    let resolution_identity = mapping
        .get("resolution_identity")
        .and_then(toml::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow!("phase8 financial envelope target gate resolution_identity is missing")
        })?;
    Ok(format!("{}.{}", resolution_kind, resolution_identity))
}

fn required_toml_nt_enum<T>(
    table: &toml::map::Map<String, toml::Value>,
    field: &'static str,
    type_name: &'static str,
) -> Result<String>
where
    T: FromStr + Display,
{
    let value = required_toml_string(table, field)?;
    canonical_loaded_toml_nt_enum::<T>(&value, field, type_name)
}

fn required_toml_integer(
    table: &toml::map::Map<String, toml::Value>,
    field: &'static str,
) -> Result<i64> {
    table
        .get(field)
        .and_then(toml::Value::as_integer)
        .ok_or_else(|| anyhow!("phase8 financial envelope loaded TOML field `{field}` is missing"))
}

fn required_toml_bool(
    table: &toml::map::Map<String, toml::Value>,
    field: &'static str,
) -> Result<bool> {
    table
        .get(field)
        .and_then(toml::Value::as_bool)
        .ok_or_else(|| anyhow!("phase8 financial envelope loaded TOML field `{field}` is missing"))
}

fn optional_toml_string(
    table: &toml::map::Map<String, toml::Value>,
    field: &'static str,
) -> Result<Option<String>> {
    match table.get(field) {
        Some(value) => value
            .as_str()
            .map(|value| Some(value.to_string()))
            .ok_or_else(|| {
                anyhow!("phase8 financial envelope loaded TOML field `{field}` must be a string")
            }),
        None => Ok(None),
    }
}

fn optional_toml_nt_enum<T>(
    table: &toml::map::Map<String, toml::Value>,
    field: &'static str,
    type_name: &'static str,
) -> Result<Option<String>>
where
    T: FromStr + Display,
{
    match table.get(field) {
        Some(value) => {
            let value = value.as_str().ok_or_else(|| {
                anyhow!("phase8 financial envelope loaded TOML field `{field}` must be a string")
            })?;
            canonical_loaded_toml_nt_enum::<T>(value, field, type_name).map(Some)
        }
        None => Ok(None),
    }
}

fn optional_toml_integer(
    table: &toml::map::Map<String, toml::Value>,
    field: &'static str,
) -> Result<Option<i64>> {
    match table.get(field) {
        Some(value) => value.as_integer().map(Some).ok_or_else(|| {
            anyhow!("phase8 financial envelope loaded TOML field `{field}` must be an integer")
        }),
        None => Ok(None),
    }
}

fn optional_toml_float(
    table: &toml::map::Map<String, toml::Value>,
    field: &'static str,
) -> Result<Option<f64>> {
    match table.get(field) {
        Some(value) => value
            .as_float()
            .or_else(|| value.as_integer().map(|integer| integer as f64))
            .map(Some)
            .ok_or_else(|| {
                anyhow!("phase8 financial envelope loaded TOML field `{field}` must be a number")
            }),
        None => Ok(None),
    }
}

#[derive(Serialize)]
struct Phase8ApprovalConsumptionEvidence<'a> {
    schema_version: i64,
    record_kind: &'static str,
    head_sha: &'a str,
    root_toml_sha256: &'a str,
    approval_envelope_sha256: &'a str,
    ssm_manifest_sha256: &'a str,
    strategy_input_evidence_sha256: &'a str,
    financial_envelope_sha256: &'a str,
    pre_run_state_sha256: &'a str,
    abort_plan_sha256: &'a str,
    approval_id_hash: String,
    approval_nonce_sha256: &'a str,
    approval_not_before_unix_secs: i64,
    approval_not_after_unix_secs: i64,
    canary_evidence_path_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    strategy_cancel_path_hash: Option<String>,
    consumed_unix_secs: i64,
}

fn required_env(name: &str) -> Result<String> {
    let value = env::var(name).map_err(|_| anyhow!("missing required phase8 env `{name}`"))?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("required phase8 env `{name}` is empty"));
    }
    Ok(trimmed.to_string())
}

fn optional_env(name: &str) -> Result<Option<String>> {
    match env::var(name) {
        Ok(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(anyhow!("failed to read phase8 env `{name}`: {error}")),
    }
}

fn required_i64_env(name: &str) -> Result<i64> {
    let value = required_env(name)?;
    value
        .parse::<i64>()
        .map_err(|source| anyhow!("failed to parse phase8 env `{name}` as i64: {source}"))
}

fn required_path_env(name: &str) -> Result<String> {
    let value = required_env(name)?;
    validate_phase8_env_path_value(name, &value)?;
    Ok(value)
}

fn optional_path_env(name: &str) -> Result<Option<String>> {
    optional_env(name)?
        .map(|value| {
            validate_phase8_env_path_value(name, &value)?;
            Ok(value)
        })
        .transpose()
}

#[cfg(test)]
fn validate_phase8_sha256_env_value(name: &str, value: String) -> Result<String> {
    if phase8_is_sha256_hex(&value) {
        Ok(value)
    } else {
        Err(anyhow!(
            "required phase8 env `{name}` must be a sha256 hash"
        ))
    }
}

fn validate_phase8_env_path_value(name: &str, value: &str) -> Result<()> {
    phase8_reject_parent_dir(value, name)
}

pub fn phase8_required_env(name: &str) -> Result<String> {
    required_env(name)
}

fn sha256_text(value: &str) -> String {
    sha256_bytes(value.as_bytes())
}

pub fn phase8_sha256_text(value: &str) -> String {
    sha256_text(value)
}

fn sha256_bytes(bytes: &[u8]) -> String {
    // Thin re-export of the ONE consolidated lowercase-hex SHA-256 primitive.
    // `hex::encode` and the prior `format!("{digest:x}")` are byte-identical for
    // a 32-byte digest, so this is behavior-preserving.
    crate::bolt_v3_source_integrity::sha256_hex_lower(bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        Phase8CanaryBlockReason, Phase8StrategyInputEvidenceFile, Phase8StrategyInputSafetyAudit,
        phase8_is_sha256_hex, phase8_resolve_configured_path, spend_phase8_approval_nonce,
        validate_phase8_env_path_value, validate_phase8_sha256_env_value,
        validate_phase8_sha256_field,
    };
    use crate::bolt_v3_decision_evidence::BoltV3GateEvidenceIdentity;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// Config-derived price-to-beat source the operator approved. The runtime evidence file's
    /// own `price_to_beat_source` must equal this for the canary to proceed.
    const TEST_EXPECTED_PRICE_TO_BEAT_SOURCE: &str = "polymarket-clob-up";
    /// A drifted runtime source that does NOT match the config-approved value above.
    const TEST_DRIFTED_PRICE_TO_BEAT_SOURCE: &str = "stale-feed";
    const TEST_SELECTED_MARKET_KEY: &str = "polymarket-binary-up-down-2026-05-30T00:00:00Z";
    const TEST_GATE_SESSION_HASH: &str = "gate-session-hash-fixture";
    const TEST_SHA256_HEX: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    /// Build a structurally valid `gate_evidence` map so
    /// `phase8_strategy_input_readiness_identity_valid` returns `true`. This is the precise
    /// precondition under which the (now-removed) self-comparison bug silently approved a
    /// drifted price-to-beat source: only when readiness identity is valid did the buggy code
    /// overwrite `expected_price_to_beat_source` with the file's own raw value.
    fn valid_gate_evidence() -> BTreeMap<String, BoltV3GateEvidenceIdentity> {
        let mut gate_evidence = BTreeMap::new();
        gate_evidence.insert(
            "resolution".to_string(),
            BoltV3GateEvidenceIdentity {
                satisfaction_kind: "evidence".to_string(),
                selected_market_key: TEST_SELECTED_MARKET_KEY.to_string(),
                provider_id: Some("polymarket-clob".to_string()),
                provider_kind: Some("clob".to_string()),
                value_kind: Some("price".to_string()),
                normalized_value_sha256: Some(TEST_SHA256_HEX.to_string()),
                provider_provenance_sha256: Some(TEST_SHA256_HEX.to_string()),
                artifact_sha256s: vec![TEST_SHA256_HEX.to_string()],
                resolution_identity: None,
            },
        );
        gate_evidence
    }

    /// Build a fully-populated evidence file whose runtime `price_to_beat_source` is the caller's
    /// choice and whose readiness identity is structurally valid (so the integrity check is the
    /// only variable under test).
    fn evidence_file_with_price_to_beat_source(
        price_to_beat_source: &str,
    ) -> Phase8StrategyInputEvidenceFile {
        Phase8StrategyInputEvidenceFile {
            strategy_instance_id: Some("strategy-instance-fixture".to_string()),
            realized_volatility: "0.25".to_string(),
            seconds_to_market_end: 3_600,
            spot_price: "0.51".to_string(),
            price_to_beat_value: "0.50".to_string(),
            expected_edge_basis_points: "120".to_string(),
            worst_case_edge_basis_points: "120".to_string(),
            fee_rate_basis_points: "10".to_string(),
            price_to_beat_source: price_to_beat_source.to_string(),
            gate_session_hash: Some(TEST_GATE_SESSION_HASH.to_string()),
            selected_market_key: Some(TEST_SELECTED_MARKET_KEY.to_string()),
            gate_evidence: Some(valid_gate_evidence()),
            reference_quote_ts_event: 1,
            pricing_kurtosis: "0.5".to_string(),
            theta_decay_factor: "0.1".to_string(),
            theta_scaled_min_edge_bps: "30".to_string(),
            market_selection_timestamp_ms: 1_700_000_001_000,
            candidate_market_start_timestamps_ms: Some(vec![1_700_000_000_000]),
            market_selection_source_path: None,
            market_selection_source_sha256: None,
            market_selection_outcome: "current".to_string(),
            polymarket_condition_id: "condition-id".to_string(),
            polymarket_market_slug: "market-slug".to_string(),
            polymarket_question_id: "question-id".to_string(),
            up_instrument_id: "up-instrument".to_string(),
            down_instrument_id: "down-instrument".to_string(),
            selected_market_observed_timestamp_ms: 1_700_000_001_000,
            polymarket_market_start_timestamp_ms: 1_700_000_000_000,
            polymarket_market_end_timestamp_ms: 1_700_000_900_000,
        }
    }

    /// Regression guard for the price-to-beat self-comparison blocker. With a structurally valid
    /// readiness identity and a runtime `price_to_beat_source` that has drifted away from the
    /// config-approved value, the audit MUST be blocked with `UnsupportedPriceToBeatSource`. The
    /// removed bug overwrote the config-derived expected source with the file's own raw value,
    /// turning the config-vs-runtime equality check into a tautology that silently approved drift.
    #[test]
    fn drifted_price_to_beat_source_is_rejected_even_with_valid_readiness_identity() {
        let raw = evidence_file_with_price_to_beat_source(TEST_DRIFTED_PRICE_TO_BEAT_SOURCE);

        let audit = Phase8StrategyInputSafetyAudit::from_raw_evidence(
            raw,
            TEST_EXPECTED_PRICE_TO_BEAT_SOURCE,
            None,
        )
        .expect("from_raw_evidence should parse a structurally valid evidence file");

        assert!(
            !audit.is_approved(),
            "a drifted runtime price_to_beat_source must never be approved; \
             block_reasons={:?}",
            audit.block_reasons()
        );
        assert!(
            audit
                .block_reasons()
                .contains(&Phase8CanaryBlockReason::UnsupportedPriceToBeatSource),
            "config-vs-runtime price_to_beat_source drift must surface \
             UnsupportedPriceToBeatSource; block_reasons={:?}",
            audit.block_reasons()
        );
        assert!(
            !audit
                .block_reasons()
                .contains(&Phase8CanaryBlockReason::DecisionEvidenceUnavailable),
            "readiness identity is structurally valid in this fixture, so the rejection must come \
             from the price-to-beat binding, not a degraded readiness identity; \
             block_reasons={:?}",
            audit.block_reasons()
        );
    }

    /// Control arm: with the SAME structurally valid readiness identity, a runtime
    /// `price_to_beat_source` that MATCHES the config-approved value must NOT trip
    /// `UnsupportedPriceToBeatSource`. This proves the integrity check is genuinely comparing
    /// config-vs-runtime (not always-fail) and isolates the drift signal in the test above.
    #[test]
    fn matching_price_to_beat_source_passes_the_integrity_check() {
        let raw = evidence_file_with_price_to_beat_source(TEST_EXPECTED_PRICE_TO_BEAT_SOURCE);

        let audit = Phase8StrategyInputSafetyAudit::from_raw_evidence(
            raw,
            TEST_EXPECTED_PRICE_TO_BEAT_SOURCE,
            None,
        )
        .expect("from_raw_evidence should parse a structurally valid evidence file");

        assert!(
            !audit
                .block_reasons()
                .contains(&Phase8CanaryBlockReason::UnsupportedPriceToBeatSource),
            "a matching config-vs-runtime price_to_beat_source must not trip the integrity check; \
             block_reasons={:?}",
            audit.block_reasons()
        );
        assert!(
            !audit
                .block_reasons()
                .contains(&Phase8CanaryBlockReason::MissingPriceToBeatSource),
            "a non-empty matching price_to_beat_source must not be flagged missing; \
             block_reasons={:?}",
            audit.block_reasons()
        );
    }

    #[test]
    fn phase8_sha256_shape_rejects_uppercase_hex() {
        let uppercase = "A".repeat(64);

        assert!(
            !phase8_is_sha256_hex(&uppercase),
            "phase8 approval evidence must use the same lowercase sha256 policy as the live gate"
        );
        assert!(
            validate_phase8_sha256_field("test_hash", &uppercase).is_err(),
            "uppercase sha256 fields must fail before live-gate consumption"
        );
    }

    #[test]
    fn phase8_sha256_env_value_rejects_uppercase_hex() {
        let uppercase = "A".repeat(64);

        let error =
            validate_phase8_sha256_env_value("BOLT_V3_PHASE8_SSM_MANIFEST_SHA256", uppercase)
                .expect_err("phase8 env sha256 values must use live-gate lowercase policy");

        assert!(
            error
                .to_string()
                .contains("BOLT_V3_PHASE8_SSM_MANIFEST_SHA256"),
            "error should name the rejected phase8 env var, got {error:?}"
        );
    }

    #[test]
    fn phase8_env_path_value_rejects_parent_dir() {
        let error = validate_phase8_env_path_value(
            "BOLT_V3_PHASE8_STRATEGY_CANCEL_PATH",
            "../strategy-cancel.json",
        )
        .expect_err("phase8 env paths must reject parent directory traversal");

        assert!(
            error
                .to_string()
                .contains("BOLT_V3_PHASE8_STRATEGY_CANCEL_PATH"),
            "error should name the rejected phase8 env var, got {error:?}"
        );
    }

    #[test]
    fn phase8_resolve_configured_path_preserves_relative_path_when_root_has_no_parent() {
        let resolved = phase8_resolve_configured_path(
            Path::new("root.toml"),
            "reports/no-submit-readiness.json",
        );

        assert_eq!(resolved, PathBuf::from("reports/no-submit-readiness.json"));
    }

    #[test]
    fn spend_phase8_approval_nonce_rewrites_the_nonce_so_old_sha_no_longer_matches() {
        // After consume, spending the nonce must overwrite the nonce file so a deliberately
        // deleted consumption marker cannot re-arm: validate_approval_nonce compares the
        // approved sha against the on-disk file, which now differs (A1/A2).
        let dir = std::env::temp_dir().join(format!("bolt-nonce-spend-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let nonce_path = dir.join("approval-nonce.json");
        std::fs::write(&nonce_path, b"original-operator-nonce-material").expect("seed nonce");

        spend_phase8_approval_nonce(
            nonce_path.to_str().expect("utf8 path"),
            "deadbeef".repeat(8).as_str(),
            1_700_000_000,
        )
        .expect("spending the nonce should succeed");

        let after = std::fs::read_to_string(&nonce_path).expect("read nonce");
        assert_ne!(
            after, "original-operator-nonce-material",
            "spending must overwrite the nonce so the approved sha256 no longer matches"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
