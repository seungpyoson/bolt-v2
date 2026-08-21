//! Gate 6 — objective `BacktestResultContract`.
//!
//! The result contract is an objective evidence/lookup artifact. It records the
//! NautilusTrader version, source-proof id/version, catalog hash, strategy
//! config hash, run purpose, fidelity class, claim limits, warnings, mechanical
//! blockers, the NautilusTrader result pointer, and artifact URIs.
//!
//! It must never encode a subjective strategy-promotion or escalation decision.
//! That is enforced structurally (there is no recommendation field) and by
//! [`BacktestResultContract::assert_objective`], which rejects promotion language
//! in any free-text field.

use std::collections::BTreeMap;

use bolt_v2::bolt_v3_config::BacktestConfigOverrideReport;
use nautilus_backtest::result::BacktestResult;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    hashing::is_lowercase_sha256_hex,
    run_manifest::StrategySource,
    seeded_l2_quote_bridge::SeededL2QuoteBridgeReport,
    seeded_level_set_deltas::{
        SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY, SEEDED_LEVEL_SET_DELTAS_TRANSFORM_VERSION,
    },
    source_proof::{AcceptanceMode, SourceProofFidelityClass},
};

/// Result contract schema version.
pub const RESULT_CONTRACT_VERSION: &str = "backtest-result-contract.v3";
const RESULT_CONTRACT_V1: &str = "backtest-result-contract.v1";
const RESULT_CONTRACT_V2: &str = "backtest-result-contract.v2";

/// Phrases that would make a result contract subjective. The contract is an
/// objective artifact; promotion/escalation belongs to Research Analytics.
const BANNED_PROMOTION_PHRASES: [&str; 8] = [
    "promote",
    "recommend",
    "escalate",
    "should use",
    "production-ready",
    "deploy this strategy",
    "winning strategy",
    "best strategy",
];

/// Resolve the verified NautilusTrader git revision this binary was built against.
///
/// The shared BVS dependency proof requires every `nautilus-*` manifest pin and
/// lockfile source to resolve to the same revision.
#[must_use]
pub fn resolved_nautilus_revision() -> Option<String> {
    crate::nt_dependency_proof::verified_nt_revision_from_embedded_manifests().ok()
}

/// Lowercase SHA-256 hex over the canonical strategy config source.
#[must_use]
pub fn strategy_config_hash(strategy: &StrategySource) -> String {
    let mut hasher = Sha256::new();
    hasher.update(strategy.registry_key.as_bytes());
    for (key, value) in &strategy.parameters {
        hasher.update([0u8]);
        hasher.update(key.as_bytes());
        hasher.update([1u8]);
        hasher.update(value.as_bytes());
    }
    if let Some(config_overlay) = &strategy.config_overlay {
        hasher.update([2u8]);
        hasher.update(
            serde_json::to_vec(config_overlay)
                .expect("strategy config overlay JSON serialization must be infallible"),
        );
    }
    hex::encode(hasher.finalize())
}

/// Objective pointer into the NautilusTrader result. Carries only mechanical
/// run facts, never a judgement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NautilusResultPointer {
    pub trader_id: String,
    pub machine_id: String,
    pub instance_id: String,
    pub run_config_id: Option<String>,
    pub backtest_start: Option<u64>,
    pub backtest_end: Option<u64>,
    pub elapsed_time_secs: f64,
    pub iterations: u64,
    pub total_events: u64,
    pub total_orders: u64,
    pub total_positions: u64,
    #[serde(default)]
    pub stats_pnls: BTreeMap<String, BTreeMap<String, f64>>,
    #[serde(default)]
    pub stats_returns: BTreeMap<String, f64>,
}

impl NautilusResultPointer {
    /// Build the pointer from a NautilusTrader [`BacktestResult`].
    #[must_use]
    pub fn from_backtest_result(result: &BacktestResult) -> Self {
        Self {
            trader_id: result.trader_id.clone(),
            machine_id: result.machine_id.clone(),
            instance_id: result.instance_id.to_string(),
            run_config_id: result.run_config_id.clone(),
            backtest_start: result.backtest_start.map(|ts| ts.as_u64()),
            backtest_end: result.backtest_end.map(|ts| ts.as_u64()),
            elapsed_time_secs: result.elapsed_time_secs,
            iterations: result.iterations as u64,
            total_events: result.total_events as u64,
            total_orders: result.total_orders as u64,
            total_positions: result.total_positions as u64,
            stats_pnls: result
                .stats_pnls
                .iter()
                .map(|(currency, stats)| {
                    (
                        currency.clone(),
                        stats
                            .iter()
                            .filter(|(_, value)| value.is_finite())
                            .map(|(name, value)| (name.clone(), *value))
                            .collect(),
                    )
                })
                .collect(),
            stats_returns: result
                .stats_returns
                .iter()
                .filter(|(_, value)| value.is_finite())
                .map(|(name, value)| (name.clone(), *value))
                .collect(),
        }
    }
}

/// Guard values used to reject a degenerate zero-result interpretation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BacktestRunGuardReport {
    pub strategy_input_snapshot_count: u64,
    pub order_intent_count: u64,
    pub admission_decision_count: u64,
    pub admitted_order_count: u64,
    pub submit_reservation_count: u64,
    pub submit_fill_count: u64,
    #[serde(default)]
    pub entry_skip_count: u64,
    #[serde(default)]
    pub exit_decision_count: u64,
    #[serde(default)]
    pub loss_governor_halt_count: u64,
    #[serde(default)]
    pub requote_throttle_count: u64,
    pub signal_quote_received: bool,
    pub realized_volatility_ready: bool,
    pub price_to_beat_received: bool,
    pub reference_fresh: bool,
    pub armed: bool,
    pub traded: bool,
    pub latest_market_id: Option<String>,
    pub latest_spot_price: Option<String>,
    pub latest_reference_current_price: Option<String>,
    pub latest_reference_current_price_source_id: Option<String>,
    pub latest_price_to_beat_value: Option<String>,
    pub latest_realized_volatility_as_of_ms: Option<u64>,
    pub latest_realized_volatility_sources_used: Vec<String>,
    pub latest_realized_volatility_blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub did_not_arm_reason: Option<String>,
}

/// Per-feed fidelity label for result interpretation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BacktestFeedLabel {
    pub feed_id: String,
    pub source_class: String,
    pub data_type: String,
    pub instrument_id: String,
    pub label: String,
}

/// Artifact URIs recorded by the contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultArtifactUris {
    pub source_proof_uri: String,
    pub canonical_table_uri: String,
    pub nt_catalog_uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nt_catalog_manifest_uri: Option<String>,
    pub catalog_metadata_uri: String,
    pub result_contract_uri: String,
}

/// The objective backtest result contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BacktestResultContract {
    pub contract_version: String,
    pub run_id: String,
    pub nt_version: String,
    pub source_proof_id: String,
    pub source_proof_version: u32,
    pub manifest_hash: String,
    pub acceptance_mode: AcceptanceMode,
    pub accepted_by: String,
    pub accepted_at: String,
    pub accepted_object_sha256: String,
    pub converter_identity: String,
    pub converter_version: String,
    pub converter_config_hash: String,
    pub conversion_manifest_hash: String,
    pub conversion_checkpoint_hash: String,
    pub catalog_hash: String,
    pub catalog_metadata_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_count_ledger_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_asset_ids_hash: Option<String>,
    pub strategy_config_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub execution_model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub venue_queue_position: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub catalog_data_types: Vec<String>,
    pub run_purpose: String,
    pub market_structure_fixture: String,
    pub fidelity_class: SourceProofFidelityClass,
    pub claim_limits: Vec<String>,
    pub warnings: Vec<String>,
    pub mechanical_blockers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_override_report: Option<BacktestConfigOverrideReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_guard_report: Option<BacktestRunGuardReport>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub feed_labels: Vec<BacktestFeedLabel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seeded_l2_quote_bridge_report: Option<SeededL2QuoteBridgeReport>,
    pub nt_result: NautilusResultPointer,
    pub artifact_uris: ResultArtifactUris,
    pub created_at: String,
}

/// Why a result contract is not objective or not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultContractError {
    MissingField(&'static str),
    InvalidSha256 { field: &'static str, value: String },
    UnsupportedVersion { actual: String },
    InvalidSeededL2QuoteBridge { detail: String },
    SubjectivePromotionLanguage { field: String, phrase: String },
}

impl std::fmt::Display for ResultContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing required field: {field}"),
            Self::InvalidSha256 { field, value } => {
                write!(
                    f,
                    "{field} must be a lowercase SHA-256 hex digest, got {value:?}"
                )
            }
            Self::UnsupportedVersion { actual } => write!(
                f,
                "unsupported result contract version: expected {RESULT_CONTRACT_VERSION}, got {actual:?}"
            ),
            Self::InvalidSeededL2QuoteBridge { detail } => {
                write!(f, "invalid seeded L2 causal quote proof: {detail}")
            }
            Self::SubjectivePromotionLanguage { field, phrase } => write!(
                f,
                "result contract field {field} contains subjective promotion language: {phrase:?}"
            ),
        }
    }
}

impl std::error::Error for ResultContractError {}

impl BacktestResultContract {
    /// Validate required fields and objectivity.
    ///
    /// # Errors
    ///
    /// Returns an error if a required field is empty or any free-text field
    /// contains promotion/escalation language.
    pub fn validate(&self) -> Result<(), ResultContractError> {
        for (name, value) in [
            ("contract_version", self.contract_version.as_str()),
            ("run_id", self.run_id.as_str()),
            ("nt_version", self.nt_version.as_str()),
            ("source_proof_id", self.source_proof_id.as_str()),
            ("manifest_hash", self.manifest_hash.as_str()),
            ("accepted_by", self.accepted_by.as_str()),
            ("accepted_at", self.accepted_at.as_str()),
            (
                "accepted_object_sha256",
                self.accepted_object_sha256.as_str(),
            ),
            ("converter_identity", self.converter_identity.as_str()),
            ("converter_version", self.converter_version.as_str()),
            ("converter_config_hash", self.converter_config_hash.as_str()),
            (
                "conversion_manifest_hash",
                self.conversion_manifest_hash.as_str(),
            ),
            (
                "conversion_checkpoint_hash",
                self.conversion_checkpoint_hash.as_str(),
            ),
            ("catalog_hash", self.catalog_hash.as_str()),
            ("catalog_metadata_hash", self.catalog_metadata_hash.as_str()),
            ("strategy_config_hash", self.strategy_config_hash.as_str()),
            ("run_purpose", self.run_purpose.as_str()),
            (
                "market_structure_fixture",
                self.market_structure_fixture.as_str(),
            ),
            ("nt_result.trader_id", self.nt_result.trader_id.as_str()),
            ("nt_result.machine_id", self.nt_result.machine_id.as_str()),
            ("nt_result.instance_id", self.nt_result.instance_id.as_str()),
            (
                "artifact_uris.source_proof_uri",
                self.artifact_uris.source_proof_uri.as_str(),
            ),
            (
                "artifact_uris.canonical_table_uri",
                self.artifact_uris.canonical_table_uri.as_str(),
            ),
            (
                "artifact_uris.nt_catalog_uri",
                self.artifact_uris.nt_catalog_uri.as_str(),
            ),
            (
                "artifact_uris.catalog_metadata_uri",
                self.artifact_uris.catalog_metadata_uri.as_str(),
            ),
            (
                "artifact_uris.result_contract_uri",
                self.artifact_uris.result_contract_uri.as_str(),
            ),
            ("created_at", self.created_at.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(ResultContractError::MissingField(name));
            }
        }
        if self.contract_version != RESULT_CONTRACT_VERSION
            && self.contract_version != RESULT_CONTRACT_V1
            && self.contract_version != RESULT_CONTRACT_V2
        {
            return Err(ResultContractError::UnsupportedVersion {
                actual: self.contract_version.clone(),
            });
        }
        for (field, value) in [
            ("manifest_hash", self.manifest_hash.as_str()),
            (
                "accepted_object_sha256",
                self.accepted_object_sha256.as_str(),
            ),
            ("converter_config_hash", self.converter_config_hash.as_str()),
            (
                "conversion_manifest_hash",
                self.conversion_manifest_hash.as_str(),
            ),
            (
                "conversion_checkpoint_hash",
                self.conversion_checkpoint_hash.as_str(),
            ),
            ("catalog_hash", self.catalog_hash.as_str()),
            ("catalog_metadata_hash", self.catalog_metadata_hash.as_str()),
            ("strategy_config_hash", self.strategy_config_hash.as_str()),
        ] {
            validate_sha256(field, value)?;
        }
        if let Some(run_config_id) = &self.nt_result.run_config_id
            && run_config_id.trim().is_empty()
        {
            return Err(ResultContractError::MissingField("nt_result.run_config_id"));
        }
        if self.claim_limits.is_empty() {
            return Err(ResultContractError::MissingField("claim_limits"));
        }
        if self.contract_version != RESULT_CONTRACT_V1 {
            if self.execution_model.trim().is_empty() {
                return Err(ResultContractError::MissingField("execution_model"));
            }
            if self.venue_queue_position.is_none() {
                return Err(ResultContractError::MissingField("venue_queue_position"));
            }
            if self.catalog_data_types.is_empty() {
                return Err(ResultContractError::MissingField("catalog_data_types"));
            }
        }
        if self.fidelity_class == SourceProofFidelityClass::L2Replay
            && self
                .event_count_ledger_hash
                .as_deref()
                .is_none_or(|hash| hash.trim().is_empty())
        {
            return Err(ResultContractError::MissingField("event_count_ledger_hash"));
        }
        if let Some(hash) = self.event_count_ledger_hash.as_deref() {
            validate_sha256("event_count_ledger_hash", hash)?;
        }
        if self.fidelity_class == SourceProofFidelityClass::L2Replay
            && self
                .selected_asset_ids_hash
                .as_deref()
                .is_none_or(|hash| hash.trim().is_empty())
        {
            return Err(ResultContractError::MissingField("selected_asset_ids_hash"));
        }
        if let Some(hash) = self.selected_asset_ids_hash.as_deref() {
            validate_sha256("selected_asset_ids_hash", hash)?;
        }
        self.validate_seeded_l2_quote_bridge_report()?;
        self.assert_objective()
    }

    fn validate_seeded_l2_quote_bridge_report(&self) -> Result<(), ResultContractError> {
        let seeded_l2_conversion =
            self.converter_identity == SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY;
        if seeded_l2_conversion
            && self.converter_version != SEEDED_LEVEL_SET_DELTAS_TRANSFORM_VERSION
        {
            return Err(ResultContractError::InvalidSeededL2QuoteBridge {
                detail: format!(
                    "seeded L2 causal proof requires the registered seeded converter version {SEEDED_LEVEL_SET_DELTAS_TRANSFORM_VERSION:?}, got {:?}",
                    self.converter_version
                ),
            });
        }
        if self.contract_version != RESULT_CONTRACT_VERSION {
            if seeded_l2_conversion || self.seeded_l2_quote_bridge_report.is_some() {
                return Err(ResultContractError::InvalidSeededL2QuoteBridge {
                    detail: format!(
                        "the registered seeded converter requires {RESULT_CONTRACT_VERSION} and a causal quote report"
                    ),
                });
            }
            return Ok(());
        }
        let report = match (
            seeded_l2_conversion,
            self.seeded_l2_quote_bridge_report.as_ref(),
        ) {
            (true, Some(report)) => report,
            (true, None) => {
                return Err(ResultContractError::InvalidSeededL2QuoteBridge {
                    detail: "seeded L2 conversion has no causal quote report".to_string(),
                });
            }
            (false, Some(_)) => {
                return Err(ResultContractError::InvalidSeededL2QuoteBridge {
                    detail: "a non-seeded conversion carries a seeded L2 causal quote report"
                        .to_string(),
                });
            }
            (false, None) => return Ok(()),
        };
        report
            .validate_for_conversion(&self.conversion_manifest_hash)
            .map_err(|error| ResultContractError::InvalidSeededL2QuoteBridge {
                detail: format!("{error:#}"),
            })
    }

    /// Reject any subjective strategy-promotion/escalation language in
    /// free-text fields.
    ///
    /// # Errors
    ///
    /// Returns the first offending field/phrase.
    pub fn assert_objective(&self) -> Result<(), ResultContractError> {
        let mut texts: Vec<(&'static str, &str)> = vec![
            ("run_purpose", self.run_purpose.as_str()),
            (
                "market_structure_fixture",
                self.market_structure_fixture.as_str(),
            ),
        ];
        for warning in &self.warnings {
            texts.push(("warnings", warning.as_str()));
        }
        for blocker in &self.mechanical_blockers {
            texts.push(("mechanical_blockers", blocker.as_str()));
        }
        for limit in &self.claim_limits {
            texts.push(("claim_limits", limit.as_str()));
        }
        for (field, text) in texts {
            let lowered = text.to_ascii_lowercase();
            for phrase in BANNED_PROMOTION_PHRASES {
                if lowered.contains(phrase) {
                    return Err(ResultContractError::SubjectivePromotionLanguage {
                        field: field.to_string(),
                        phrase: phrase.to_string(),
                    });
                }
            }
        }
        Ok(())
    }
}

fn validate_sha256(field: &'static str, value: &str) -> Result<(), ResultContractError> {
    if is_lowercase_sha256_hex(value) {
        Ok(())
    } else {
        Err(ResultContractError::InvalidSha256 {
            field,
            value: value.to_string(),
        })
    }
}

/// Inputs assembled by the runner to build a [`BacktestResultContract`].
pub struct ResultContractInputs<'a> {
    pub run_id: &'a str,
    pub source_proof_id: &'a str,
    pub source_proof_version: u32,
    pub manifest_hash: &'a str,
    pub acceptance_mode: AcceptanceMode,
    pub accepted_by: &'a str,
    pub accepted_at: &'a str,
    pub accepted_object_sha256: &'a str,
    pub converter_identity: &'a str,
    pub converter_version: &'a str,
    pub converter_config_hash: &'a str,
    pub conversion_manifest_hash: &'a str,
    pub conversion_checkpoint_hash: &'a str,
    pub catalog_hash: &'a str,
    pub catalog_metadata_hash: &'a str,
    pub event_count_ledger_hash: Option<&'a str>,
    pub selected_asset_ids_hash: Option<&'a str>,
    pub strategy: &'a StrategySource,
    pub execution_model: &'a str,
    pub venue_queue_position: bool,
    pub catalog_data_types: Vec<String>,
    pub run_purpose: &'a str,
    pub market_structure_fixture: &'a str,
    pub fidelity_class: SourceProofFidelityClass,
    pub claim_limits: Vec<String>,
    pub warnings: Vec<String>,
    pub mechanical_blockers: Vec<String>,
    pub config_override_report: Option<&'a BacktestConfigOverrideReport>,
    pub run_guard_report: Option<&'a BacktestRunGuardReport>,
    pub feed_labels: Vec<BacktestFeedLabel>,
    pub seeded_l2_quote_bridge_report: Option<&'a SeededL2QuoteBridgeReport>,
    pub nt_result: &'a BacktestResult,
    pub artifact_uris: ResultArtifactUris,
    pub created_at: &'a str,
}

/// Build an objective result contract from runner inputs.
///
/// # Errors
///
/// Returns an error if the NautilusTrader revision cannot be resolved or the
/// contract fails objectivity/completeness validation.
pub fn build_result_contract(
    inputs: ResultContractInputs<'_>,
) -> Result<BacktestResultContract, ResultContractError> {
    let nt_version =
        resolved_nautilus_revision().ok_or(ResultContractError::MissingField("nt_version"))?;
    let contract = BacktestResultContract {
        contract_version: RESULT_CONTRACT_VERSION.to_string(),
        run_id: inputs.run_id.to_string(),
        nt_version,
        source_proof_id: inputs.source_proof_id.to_string(),
        source_proof_version: inputs.source_proof_version,
        manifest_hash: inputs.manifest_hash.to_string(),
        acceptance_mode: inputs.acceptance_mode,
        accepted_by: inputs.accepted_by.to_string(),
        accepted_at: inputs.accepted_at.to_string(),
        accepted_object_sha256: inputs.accepted_object_sha256.to_string(),
        converter_identity: inputs.converter_identity.to_string(),
        converter_version: inputs.converter_version.to_string(),
        converter_config_hash: inputs.converter_config_hash.to_string(),
        conversion_manifest_hash: inputs.conversion_manifest_hash.to_string(),
        conversion_checkpoint_hash: inputs.conversion_checkpoint_hash.to_string(),
        catalog_hash: inputs.catalog_hash.to_string(),
        catalog_metadata_hash: inputs.catalog_metadata_hash.to_string(),
        event_count_ledger_hash: inputs.event_count_ledger_hash.map(str::to_string),
        selected_asset_ids_hash: inputs.selected_asset_ids_hash.map(str::to_string),
        strategy_config_hash: strategy_config_hash(inputs.strategy),
        execution_model: inputs.execution_model.to_string(),
        venue_queue_position: Some(inputs.venue_queue_position),
        catalog_data_types: inputs.catalog_data_types,
        run_purpose: inputs.run_purpose.to_string(),
        market_structure_fixture: inputs.market_structure_fixture.to_string(),
        fidelity_class: inputs.fidelity_class,
        claim_limits: inputs.claim_limits,
        warnings: inputs.warnings,
        mechanical_blockers: inputs.mechanical_blockers,
        config_override_report: inputs.config_override_report.cloned(),
        run_guard_report: inputs.run_guard_report.cloned(),
        feed_labels: inputs.feed_labels,
        seeded_l2_quote_bridge_report: inputs.seeded_l2_quote_bridge_report.cloned(),
        nt_result: NautilusResultPointer::from_backtest_result(inputs.nt_result),
        artifact_uris: inputs.artifact_uris,
        created_at: inputs.created_at.to_string(),
    };
    contract.validate()?;
    Ok(contract)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pointer() -> NautilusResultPointer {
        NautilusResultPointer {
            trader_id: "BACKTESTER-001".to_string(),
            machine_id: "host".to_string(),
            instance_id: "11111111-1111-1111-1111-111111111111".to_string(),
            run_config_id: Some("run".to_string()),
            backtest_start: Some(1),
            backtest_end: Some(2),
            elapsed_time_secs: 0.1,
            iterations: 937,
            total_events: 937,
            total_orders: 0,
            total_positions: 0,
            stats_pnls: BTreeMap::new(),
            stats_returns: BTreeMap::new(),
        }
    }

    const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const HASH_C: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const HASH_D: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    const HASH_E: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    const HASH_F: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    const HASH_0: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const HASH_1: &str = "1111111111111111111111111111111111111111111111111111111111111111";
    const HASH_2: &str = "2222222222222222222222222222222222222222222222222222222222222222";

    fn contract() -> BacktestResultContract {
        BacktestResultContract {
            contract_version: RESULT_CONTRACT_VERSION.to_string(),
            run_id: "run".to_string(),
            nt_version: crate::nt_dependency_proof::verified_nt_revision_from_embedded_manifests()
                .expect("BVS NautilusTrader dependency provenance"),
            source_proof_id: "source-proof-bybit-spot-tick-trades".to_string(),
            source_proof_version: 1,
            manifest_hash: HASH_A.to_string(),
            acceptance_mode: AcceptanceMode::Manual,
            accepted_by: "vertical-slice-operator".to_string(),
            accepted_at: "2026-06-02T00:00:00Z".to_string(),
            accepted_object_sha256:
                "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598".to_string(),
            converter_identity: "csv-native-trades-to-canonical-trades.v1".to_string(),
            converter_version: "1".to_string(),
            converter_config_hash: HASH_B.to_string(),
            conversion_manifest_hash: HASH_C.to_string(),
            conversion_checkpoint_hash: HASH_D.to_string(),
            catalog_hash: HASH_E.to_string(),
            catalog_metadata_hash: HASH_F.to_string(),
            event_count_ledger_hash: None,
            selected_asset_ids_hash: None,
            strategy_config_hash: HASH_0.to_string(),
            execution_model: "nt_backtest_node".to_string(),
            venue_queue_position: Some(false),
            catalog_data_types: vec!["TradeTick".to_string()],
            run_purpose: "normal".to_string(),
            market_structure_fixture: "perps-spot".to_string(),
            fidelity_class: SourceProofFidelityClass::TradeReplay,
            claim_limits: vec![
                "No execution-quality, queue-position, or order-book-liquidity claims.".to_string(),
            ],
            warnings: vec![
                "No orders placed: trade-only data has no quote ticks and the strategy's entry \
                 is quote-driven."
                    .to_string(),
            ],
            mechanical_blockers: vec![],
            config_override_report: None,
            run_guard_report: None,
            feed_labels: vec![],
            seeded_l2_quote_bridge_report: None,
            nt_result: pointer(),
            artifact_uris: ResultArtifactUris {
                source_proof_uri: "s3://.../source-proofs/p.json".to_string(),
                canonical_table_uri: "s3://.../trades.parquet".to_string(),
                nt_catalog_uri: "s3://.../nt-catalog/".to_string(),
                nt_catalog_manifest_uri: None,
                catalog_metadata_uri: "s3://.../catalog-metadata.json".to_string(),
                result_contract_uri: "s3://.../backtests/run/result.json".to_string(),
            },
            created_at: "2026-06-02T00:00:00Z".to_string(),
        }
    }

    fn seeded_contract() -> BacktestResultContract {
        let mut contract = contract();
        contract.converter_identity =
            crate::seeded_level_set_deltas::SEEDED_LEVEL_SET_DELTAS_TRANSFORM_IDENTITY.to_string();
        contract.converter_version =
            crate::seeded_level_set_deltas::SEEDED_LEVEL_SET_DELTAS_TRANSFORM_VERSION.to_string();
        contract.catalog_data_types = vec!["OrderBookDelta".to_string()];
        contract.fidelity_class = SourceProofFidelityClass::L2Replay;
        contract.event_count_ledger_hash = Some(HASH_1.to_string());
        contract.selected_asset_ids_hash = Some(HASH_2.to_string());
        contract
    }

    fn seeded_l2_report() -> SeededL2QuoteBridgeReport {
        SeededL2QuoteBridgeReport {
            schema_version: "seeded-l2-quote-bridge-report.v1".to_string(),
            plan_hash: HASH_1.to_string(),
            instruments: vec![
                crate::seeded_l2_quote_bridge::SeededL2QuoteBridgeInstrumentReport {
                    nt_instrument_id: "BTC-USDT.OKX".to_string(),
                    conversion_manifest_hash: HASH_C.to_string(),
                    observed_event_batches: 2,
                    observed_source_events: 2,
                    observed_delta_rows: 4,
                    emitted_quotes: 2,
                    causal_trace_hash: HASH_2.to_string(),
                },
            ],
        }
    }

    #[test]
    fn resolved_revision_is_a_git_sha() {
        let rev = resolved_nautilus_revision().expect("revision");
        assert_eq!(rev.len(), 40);
        assert!(rev.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn objective_contract_validates() {
        assert_eq!(RESULT_CONTRACT_VERSION, "backtest-result-contract.v3");
        contract().validate().expect("objective contract is valid");
    }

    #[test]
    fn seeded_converter_requires_and_accepts_its_causal_quote_proof() {
        let missing = seeded_contract()
            .validate()
            .expect_err("seeded L2 conversion must carry its causal quote proof");
        assert!(
            matches!(
                missing,
                ResultContractError::InvalidSeededL2QuoteBridge { .. }
            ),
            "{missing}"
        );

        let mut valid = seeded_contract();
        valid.seeded_l2_quote_bridge_report = Some(seeded_l2_report());
        valid
            .validate()
            .expect("current result contract accepts a valid causal quote proof");

        let mut unrelated = contract();
        unrelated.seeded_l2_quote_bridge_report = Some(seeded_l2_report());
        assert!(matches!(
            unrelated.validate(),
            Err(ResultContractError::InvalidSeededL2QuoteBridge { .. })
        ));

        let mut historical = seeded_contract();
        historical.contract_version = RESULT_CONTRACT_V2.to_string();
        historical.seeded_l2_quote_bridge_report = Some(seeded_l2_report());
        assert!(matches!(
            historical.validate(),
            Err(ResultContractError::InvalidSeededL2QuoteBridge { .. })
        ));
    }

    #[test]
    fn historical_contract_cannot_be_promoted_by_binding_a_seeded_l2_proof() {
        let mut historical = seeded_contract();
        historical.contract_version = RESULT_CONTRACT_V2.to_string();
        historical.seeded_l2_quote_bridge_report = Some(seeded_l2_report());

        assert!(matches!(
            historical.validate(),
            Err(ResultContractError::InvalidSeededL2QuoteBridge { .. })
        ));
    }

    #[test]
    fn current_seeded_contract_cannot_downgrade_away_its_causal_proof() {
        let mut downgraded = seeded_contract();
        downgraded.contract_version = RESULT_CONTRACT_V2.to_string();

        let error = downgraded
            .validate()
            .expect_err("a schema-label downgrade must not remove causal proof authority");
        assert!(
            error
                .to_string()
                .contains("requires backtest-result-contract.v3"),
            "{error}"
        );
    }

    #[test]
    fn seeded_l2_proof_rejects_source_events_exceeding_batches() {
        let mut c = seeded_contract();
        let mut report = seeded_l2_report();
        report.instruments[0].observed_event_batches = 1;
        report.instruments[0].observed_source_events = 2;
        c.seeded_l2_quote_bridge_report = Some(report);

        let error = c
            .validate()
            .expect_err("source events cannot exceed batches");
        assert!(
            error
                .to_string()
                .contains("source-event count exceeds observed event batches"),
            "{error}"
        );
    }

    #[test]
    fn seeded_l2_proof_rejects_more_quotes_than_source_events() {
        let mut c = seeded_contract();
        let mut report = seeded_l2_report();
        report.instruments[0].emitted_quotes = 3;
        c.seeded_l2_quote_bridge_report = Some(report);

        let error = c
            .validate()
            .expect_err("quotes cannot exceed source events");
        assert!(
            error
                .to_string()
                .contains("emitted more quotes than source events"),
            "{error}"
        );
    }

    #[test]
    fn seeded_l2_proof_rejects_duplicate_instruments() {
        let mut c = seeded_contract();
        let mut report = seeded_l2_report();
        report.instruments.push(report.instruments[0].clone());
        c.seeded_l2_quote_bridge_report = Some(report);

        let error = c.validate().expect_err("duplicate instruments must fail");
        assert!(
            error.to_string().contains("duplicate report instrument"),
            "{error}"
        );
    }

    #[test]
    fn seeded_l2_proof_requires_the_exact_registered_converter_version() {
        let mut c = seeded_contract();
        c.converter_version = "seeded-level-set-deltas.v999".to_string();
        c.seeded_l2_quote_bridge_report = Some(seeded_l2_report());

        let error = c
            .validate()
            .expect_err("an unregistered seeded converter version must fail");
        assert!(
            error
                .to_string()
                .contains("requires the registered seeded converter version"),
            "{error}"
        );
    }

    #[test]
    fn strategy_config_hash_covers_config_overlay_delta() {
        let mut strategy = crate::run_manifest::StrategySource {
            source_kind: crate::run_manifest::StrategySourceKind::CompiledRustRegistry,
            registry_key: "binary_oracle_edge_taker".to_string(),
            parameters: BTreeMap::new(),
            typed_config_uri: None,
            typed_config_hash: None,
            experiment_result_uri: None,
            experiment_result_hash: None,
            config_overlay: Some(crate::run_manifest::StrategyConfigOverlaySource {
                override_delta: crate::run_manifest::ManifestBacktestConfigOverride {
                    label: "production config + documented OKX/Bybit override".to_string(),
                    strategy_instance_id: "binary_oracle_btc".to_string(),
                    signal_role: "primary".to_string(),
                    signal_data_client_id: "okx_data".to_string(),
                    signal_instrument_id: "BTC-USDT.OKX".to_string(),
                    realized_volatility_surface_id: "btc_usdt_midpoint_rv".to_string(),
                    keep_realized_volatility_sources: vec![
                        crate::run_manifest::ManifestRealizedVolatilitySourceSelector {
                            data_client_id: "okx_data".to_string(),
                            instrument_id: "BTC-USDT.OKX".to_string(),
                        },
                    ],
                },
            }),
        };
        let base = strategy_config_hash(&strategy);
        strategy
            .config_overlay
            .as_mut()
            .expect("overlay")
            .override_delta
            .signal_instrument_id = "BTC-USDT.BYBIT".to_string();

        assert_ne!(base, strategy_config_hash(&strategy));
    }

    #[test]
    fn result_contract_binds_manifest_and_acceptance_provenance() {
        let c = contract();

        assert_eq!(c.manifest_hash, HASH_A);
        assert_eq!(c.acceptance_mode, AcceptanceMode::Manual);
        assert_eq!(c.accepted_by, "vertical-slice-operator");
        assert_eq!(c.accepted_at, "2026-06-02T00:00:00Z");
        assert_eq!(
            c.accepted_object_sha256,
            "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598"
        );
        assert_eq!(
            c.converter_identity,
            "csv-native-trades-to-canonical-trades.v1"
        );
        assert_eq!(c.converter_version, "1");
        assert_eq!(c.converter_config_hash, HASH_B);
        assert_eq!(c.conversion_manifest_hash, HASH_C);
        assert_eq!(c.conversion_checkpoint_hash, HASH_D);
        assert_eq!(c.catalog_metadata_hash, HASH_F);
        assert_eq!(
            c.artifact_uris.catalog_metadata_uri,
            "s3://.../catalog-metadata.json"
        );

        let json = serde_json::to_value(&c).expect("serialize");
        for field in [
            "manifest_hash",
            "acceptance_mode",
            "accepted_by",
            "accepted_at",
            "accepted_object_sha256",
            "converter_identity",
            "converter_version",
            "converter_config_hash",
            "conversion_manifest_hash",
            "conversion_checkpoint_hash",
            "catalog_metadata_hash",
            "execution_model",
            "venue_queue_position",
            "catalog_data_types",
        ] {
            assert!(
                json.get(field).is_some(),
                "missing serialized field {field}"
            );
        }
    }

    #[test]
    fn rejects_promotion_language_in_warnings() {
        let mut c = contract();
        c.warnings
            .push("We recommend you promote this strategy to production.".to_string());
        let err = c.assert_objective().unwrap_err();
        assert!(matches!(
            err,
            ResultContractError::SubjectivePromotionLanguage { .. }
        ));
    }

    #[test]
    fn rejects_promotion_language_in_claim_limits_and_blockers() {
        for phrase in BANNED_PROMOTION_PHRASES {
            let mut c = contract();
            c.claim_limits.push(format!("note: {phrase} later"));
            assert!(
                matches!(
                    c.assert_objective(),
                    Err(ResultContractError::SubjectivePromotionLanguage { ref field, .. })
                        if field == "claim_limits"
                ),
                "claim_limits should reject {phrase:?}"
            );

            let mut c = contract();
            c.mechanical_blockers.push(format!("blocked: {phrase}"));
            assert!(
                matches!(
                    c.assert_objective(),
                    Err(ResultContractError::SubjectivePromotionLanguage { ref field, .. })
                        if field == "mechanical_blockers"
                ),
                "mechanical_blockers should reject {phrase:?}"
            );
        }
    }

    #[test]
    fn benign_language_passes_objectivity() {
        let mut c = contract();
        c.warnings
            .push("Data is trade-only; coverage is one day.".to_string());
        c.mechanical_blockers
            .push("Run halted: catalog read-back count mismatch.".to_string());
        c.assert_objective()
            .expect("benign operational language must pass");
    }

    #[test]
    fn rejects_promotion_language_in_run_purpose() {
        let mut c = contract();
        c.run_purpose = "promote".to_string();
        assert!(
            matches!(
                c.assert_objective(),
                Err(ResultContractError::SubjectivePromotionLanguage { ref field, .. })
                    if field == "run_purpose"
            ),
            "run_purpose must be scanned for promotion language"
        );
    }

    #[test]
    fn rejects_promotion_language_in_market_structure_fixture() {
        let mut c = contract();
        c.market_structure_fixture = "best strategy".to_string();
        assert!(
            matches!(
                c.assert_objective(),
                Err(ResultContractError::SubjectivePromotionLanguage { ref field, .. })
                    if field == "market_structure_fixture"
            ),
            "market_structure_fixture must be scanned for promotion language"
        );
    }

    #[test]
    fn rejects_missing_claim_limits() {
        let mut c = contract();
        c.claim_limits.clear();
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("claim_limits")
        );
    }

    #[test]
    fn v2_result_contract_requires_manifest_execution_evidence() {
        let mut c = contract();
        c.execution_model.clear();
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("execution_model")
        );

        let mut c = contract();
        c.venue_queue_position = None;
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("venue_queue_position")
        );

        let mut c = contract();
        c.catalog_data_types.clear();
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("catalog_data_types")
        );
    }

    #[test]
    fn v1_result_contract_without_manifest_execution_evidence_still_deserializes() {
        let mut value = serde_json::to_value(contract()).expect("serialize");
        value["contract_version"] = serde_json::json!(RESULT_CONTRACT_V1);
        value.as_object_mut().unwrap().remove("execution_model");
        value
            .as_object_mut()
            .unwrap()
            .remove("venue_queue_position");
        value.as_object_mut().unwrap().remove("catalog_data_types");

        let parsed: BacktestResultContract =
            serde_json::from_value(value).expect("deserialize v1 contract");
        parsed
            .validate()
            .expect("v1 contract remains readable as historical evidence");
    }

    #[test]
    fn l2_result_contract_requires_event_count_ledger_hash() {
        let mut c = contract();
        c.fidelity_class = SourceProofFidelityClass::L2Replay;
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("event_count_ledger_hash")
        );
    }

    #[test]
    fn l2_result_contract_requires_and_binds_selected_asset_ids_hash() {
        let mut c = contract();
        c.fidelity_class = SourceProofFidelityClass::L2Replay;
        c.event_count_ledger_hash = Some(HASH_1.to_string());
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("selected_asset_ids_hash")
        );

        c.selected_asset_ids_hash = Some(HASH_2.to_string());
        c.validate()
            .expect("L2 contract with selector hashes is complete");
        let json = serde_json::to_value(&c).expect("serialize");
        assert_eq!(
            json.get("event_count_ledger_hash").and_then(|v| v.as_str()),
            Some(HASH_1)
        );
        assert_eq!(
            json.get("selected_asset_ids_hash").and_then(|v| v.as_str()),
            Some(HASH_2)
        );
    }

    #[test]
    fn rejects_malformed_hash_fields() {
        type HashMutator = fn(&mut BacktestResultContract, &str);
        let cases: [(&str, HashMutator, &str); 3] = [
            (
                "manifest_hash",
                |c: &mut BacktestResultContract, value: &str| {
                    c.manifest_hash = value.to_string();
                },
                "abc123",
            ),
            (
                "converter_config_hash",
                |c: &mut BacktestResultContract, value: &str| {
                    c.converter_config_hash = value.to_string();
                },
                "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            ),
            (
                "catalog_metadata_hash",
                |c: &mut BacktestResultContract, value: &str| {
                    c.catalog_metadata_hash = value.to_string();
                },
                "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
            ),
        ];
        for (field, mutate, value) in cases {
            let mut c = contract();
            mutate(&mut c, value);
            assert_eq!(
                c.validate().unwrap_err(),
                ResultContractError::InvalidSha256 {
                    field,
                    value: value.to_string()
                }
            );
        }
    }

    #[test]
    fn l2_result_contract_rejects_malformed_selector_hashes() {
        let mut c = contract();
        c.fidelity_class = SourceProofFidelityClass::L2Replay;
        c.event_count_ledger_hash = Some("abc123".to_string());
        c.selected_asset_ids_hash = Some(HASH_2.to_string());
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::InvalidSha256 {
                field: "event_count_ledger_hash",
                value: "abc123".to_string()
            }
        );
    }

    #[test]
    fn rejects_missing_contract_version() {
        let mut c = contract();
        c.contract_version.clear();
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("contract_version")
        );
    }

    #[test]
    fn rejects_unsupported_contract_version() {
        let mut c = contract();
        c.contract_version = "backtest-result-contract.v999".to_string();
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::UnsupportedVersion {
                actual: "backtest-result-contract.v999".to_string()
            }
        );
    }

    #[test]
    fn rejects_unknown_contract_fields() {
        let c = contract();
        let mut value = serde_json::to_value(&c).expect("serialize");
        value
            .as_object_mut()
            .expect("contract object")
            .insert("future_schema_field".to_string(), serde_json::json!(true));
        assert!(
            serde_json::from_value::<BacktestResultContract>(value).is_err(),
            "top-level unknown result contract fields must fail closed"
        );

        let mut value = serde_json::to_value(&c).expect("serialize");
        value
            .get_mut("artifact_uris")
            .and_then(serde_json::Value::as_object_mut)
            .expect("artifact_uris object")
            .insert(
                "future_artifact_uri".to_string(),
                serde_json::json!("s3://example/future.json"),
            );
        assert!(
            serde_json::from_value::<BacktestResultContract>(value).is_err(),
            "nested unknown result contract fields must fail closed"
        );
    }

    #[test]
    fn rejects_missing_run_purpose() {
        let mut c = contract();
        c.run_purpose.clear();
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("run_purpose")
        );
    }

    #[test]
    fn rejects_missing_market_structure_fixture() {
        let mut c = contract();
        c.market_structure_fixture.clear();
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("market_structure_fixture")
        );
    }

    #[test]
    fn rejects_missing_artifact_uri() {
        let mut c = contract();
        c.artifact_uris.result_contract_uri.clear();
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("artifact_uris.result_contract_uri")
        );
    }

    #[test]
    fn rejects_missing_nt_pointer_identity() {
        let mut c = contract();
        c.nt_result.trader_id.clear();
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("nt_result.trader_id")
        );
    }

    #[test]
    fn round_trips_through_json() {
        let c = contract();
        let json = serde_json::to_string_pretty(&c).expect("serialize");
        let parsed: BacktestResultContract = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, c);
    }
}
