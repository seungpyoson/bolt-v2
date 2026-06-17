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

use nautilus_backtest::result::BacktestResult;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    run_manifest::StrategySource,
    source_proof::{AcceptanceMode, SourceProofFidelityClass},
};

/// Result contract schema version.
pub const RESULT_CONTRACT_VERSION: &str = "backtest-result-contract.v1";

/// This crate's manifest, embedded at compile time so the recorded NautilusTrader
/// revision is exactly the one this binary was built against. This crate's own
/// `Cargo.toml` is the single source of truth for the pinned `nautilus-backtest`
/// rev (the slice roots its own workspace + lockfile, isolated from `bolt-v2`).
const WORKSPACE_CARGO_TOML: &str = include_str!("../Cargo.toml");

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

/// Resolve the NautilusTrader git revision this binary was built against, read
/// from the embedded workspace `Cargo.toml` (single source of truth).
#[must_use]
pub fn resolved_nautilus_revision() -> Option<String> {
    nautilus_revision_from_manifest(WORKSPACE_CARGO_TOML)
}

fn nautilus_revision_from_manifest(manifest: &str) -> Option<String> {
    let parsed = toml::from_str::<toml::Table>(manifest).ok()?;
    let rev = parsed
        .get("dependencies")?
        .as_table()?
        .get("nautilus-backtest")?
        .as_table()?
        .get("rev")?
        .as_str()?;
    if rev.len() == 40 && rev.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(rev.to_string())
    } else {
        None
    }
}

/// Lowercase SHA-256 hex over the canonical strategy config (registry key plus
/// sorted parameters).
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
    hex::encode(hasher.finalize())
}

/// Objective pointer into the NautilusTrader result. Carries only mechanical
/// run facts, never a judgement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        }
    }
}

/// Artifact URIs recorded by the contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultArtifactUris {
    pub source_proof_uri: String,
    pub canonical_table_uri: String,
    pub nt_catalog_uri: String,
    pub catalog_metadata_uri: String,
    pub result_contract_uri: String,
}

/// The objective backtest result contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    pub run_purpose: String,
    pub market_structure_fixture: String,
    pub fidelity_class: SourceProofFidelityClass,
    pub claim_limits: Vec<String>,
    pub warnings: Vec<String>,
    pub mechanical_blockers: Vec<String>,
    pub nt_result: NautilusResultPointer,
    pub artifact_uris: ResultArtifactUris,
    pub created_at: String,
}

/// Why a result contract is not objective or not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultContractError {
    MissingField(&'static str),
    SubjectivePromotionLanguage { field: String, phrase: String },
}

impl std::fmt::Display for ResultContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "missing required field: {field}"),
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
        if let Some(run_config_id) = &self.nt_result.run_config_id
            && run_config_id.trim().is_empty()
        {
            return Err(ResultContractError::MissingField("nt_result.run_config_id"));
        }
        if self.claim_limits.is_empty() {
            return Err(ResultContractError::MissingField("claim_limits"));
        }
        if self.fidelity_class == SourceProofFidelityClass::L2Replay
            && self
                .event_count_ledger_hash
                .as_deref()
                .is_none_or(|hash| hash.trim().is_empty())
        {
            return Err(ResultContractError::MissingField("event_count_ledger_hash"));
        }
        if self.fidelity_class == SourceProofFidelityClass::L2Replay
            && self
                .selected_asset_ids_hash
                .as_deref()
                .is_none_or(|hash| hash.trim().is_empty())
        {
            return Err(ResultContractError::MissingField("selected_asset_ids_hash"));
        }
        self.assert_objective()
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
    pub run_purpose: &'a str,
    pub market_structure_fixture: &'a str,
    pub fidelity_class: SourceProofFidelityClass,
    pub claim_limits: Vec<String>,
    pub warnings: Vec<String>,
    pub mechanical_blockers: Vec<String>,
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
        run_purpose: inputs.run_purpose.to_string(),
        market_structure_fixture: inputs.market_structure_fixture.to_string(),
        fidelity_class: inputs.fidelity_class,
        claim_limits: inputs.claim_limits,
        warnings: inputs.warnings,
        mechanical_blockers: inputs.mechanical_blockers,
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
        }
    }

    fn contract() -> BacktestResultContract {
        BacktestResultContract {
            contract_version: RESULT_CONTRACT_VERSION.to_string(),
            run_id: "run".to_string(),
            nt_version: "6be5a5094716790a8ca2875445fde4fa2586107e".to_string(),
            source_proof_id: "source-proof-bybit-spot-tick-trades".to_string(),
            source_proof_version: 1,
            manifest_hash: "manifestabc".to_string(),
            acceptance_mode: AcceptanceMode::Manual,
            accepted_by: "vertical-slice-operator".to_string(),
            accepted_at: "2026-06-02T00:00:00Z".to_string(),
            accepted_object_sha256:
                "d6af93305f3773d6c00b4f3c13ffaef54a573d62ce5e6a96649b06d82df04598".to_string(),
            converter_identity: "csv-native-trades-to-canonical-trades.v1".to_string(),
            converter_version: "1".to_string(),
            converter_config_hash: "converterconfigabc".to_string(),
            conversion_manifest_hash: "conversionmanifestabc".to_string(),
            conversion_checkpoint_hash: "conversioncheckpointabc".to_string(),
            catalog_hash: "abc123".to_string(),
            catalog_metadata_hash: "metahashabc".to_string(),
            event_count_ledger_hash: None,
            selected_asset_ids_hash: None,
            strategy_config_hash: "def456".to_string(),
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
            nt_result: pointer(),
            artifact_uris: ResultArtifactUris {
                source_proof_uri: "s3://.../source-proofs/p.json".to_string(),
                canonical_table_uri: "s3://.../trades.parquet".to_string(),
                nt_catalog_uri: "s3://.../nt-catalog/".to_string(),
                catalog_metadata_uri: "s3://.../catalog-metadata.json".to_string(),
                result_contract_uri: "s3://.../backtests/run/result.json".to_string(),
            },
            created_at: "2026-06-02T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn resolved_revision_is_a_git_sha() {
        let rev = resolved_nautilus_revision().expect("revision");
        assert_eq!(rev.len(), 40);
        assert!(rev.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn parses_revision_from_multiline_dependency_table() {
        let manifest = r#"[dependencies.nautilus-backtest]
git = "https://github.com/nautechsystems/nautilus_trader.git"
rev = "6be5a5094716790a8ca2875445fde4fa2586107e"
features = ["streaming", "examples"]
"#;
        assert_eq!(
            nautilus_revision_from_manifest(manifest).as_deref(),
            Some("6be5a5094716790a8ca2875445fde4fa2586107e")
        );
    }

    #[test]
    fn objective_contract_validates() {
        contract().validate().expect("objective contract is valid");
    }

    #[test]
    fn result_contract_binds_manifest_and_acceptance_provenance() {
        let c = contract();

        assert_eq!(c.manifest_hash, "manifestabc");
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
        assert_eq!(c.converter_config_hash, "converterconfigabc");
        assert_eq!(c.conversion_manifest_hash, "conversionmanifestabc");
        assert_eq!(c.conversion_checkpoint_hash, "conversioncheckpointabc");
        assert_eq!(c.catalog_metadata_hash, "metahashabc");
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
        c.event_count_ledger_hash = Some("eventledgerabc".to_string());
        assert_eq!(
            c.validate().unwrap_err(),
            ResultContractError::MissingField("selected_asset_ids_hash")
        );

        c.selected_asset_ids_hash = Some("selectedassetsabc".to_string());
        c.validate()
            .expect("L2 contract with selector hashes is complete");
        let json = serde_json::to_value(&c).expect("serialize");
        assert_eq!(
            json.get("event_count_ledger_hash").and_then(|v| v.as_str()),
            Some("eventledgerabc")
        );
        assert_eq!(
            json.get("selected_asset_ids_hash").and_then(|v| v.as_str()),
            Some("selectedassetsabc")
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
