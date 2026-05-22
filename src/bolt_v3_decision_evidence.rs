use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, anyhow};
use serde::Serialize;

use crate::bolt_v3_config::LoadedBoltV3Config;

pub const BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION: u32 = 3;
pub const BOLT_V3_DECISION_EVIDENCE_GATE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BOLT_V3_ORDER_INTENT_GATE_ID: &str = "bolt_v3.order_intent";
pub const BOLT_V3_SUBMIT_ADMISSION_GATE_ID: &str = "bolt_v3.submit_admission";
pub const BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID: &str = "bolt_v3.strategy_input_snapshot";
pub const BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_CURRENT: &str = "current";

pub trait BoltV3DecisionEvidenceWriter: std::fmt::Debug + Send + Sync {
    fn record_strategy_input_snapshot(
        &self,
        snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()>;

    fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()>;
    fn record_admission_decision(&self, decision: &BoltV3AdmissionDecisionEvidence) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3OrderIntentKind {
    Entry,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3SubmitIntentKind {
    Entry,
    RiskReducingExit,
    ReplaceSubmit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3OrderIntentEvidence {
    pub strategy_id: String,
    pub intent_kind: BoltV3OrderIntentKind,
    pub instrument_id: String,
    pub client_order_id: String,
    pub order_side: String,
    pub price: String,
    pub quantity: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3StrategyInputEvidenceSnapshot {
    pub strategy_id: String,
    pub configured_target_id: String,
    pub market_selection_ruleset_id: String,
    pub market_selection_outcome: String,
    pub market_id: Option<String>,
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
    pub reference_fair_value: Option<String>,
    pub realized_volatility: String,
    pub seconds_to_market_end: u64,
    pub pricing_kurtosis: String,
    pub theta_decay_factor: String,
    pub theta_scaled_min_edge_bps: String,
    pub fair_probability_up: String,
    pub uncertainty_band_probability: String,
    pub expected_edge_basis_points: String,
    pub worst_case_edge_basis_points: String,
    pub fee_rate_basis_points: String,
    pub selected_side: Option<String>,
    pub submission_instrument_id: String,
    pub submission_order_side: String,
    pub submission_price: String,
    pub submission_quantity: String,
    pub client_order_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3AdmissionOutcome {
    Admitted,
    RejectedNotArmed,
    RejectedSubmitLifecycleDisallowed,
    RejectedNonPositiveNotional,
    RejectedNotionalCapExceeded,
    RejectedCountCapExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3AdmissionDecisionEvidence {
    pub strategy_id: String,
    pub client_order_id: String,
    pub instrument_id: String,
    pub notional: String,
    pub intent_kind: BoltV3SubmitIntentKind,
    pub outcome: BoltV3AdmissionOutcome,
}

#[derive(Debug)]
pub struct JsonlBoltV3DecisionEvidenceWriter {
    file: Mutex<std::fs::File>,
}

impl JsonlBoltV3DecisionEvidenceWriter {
    pub fn from_loaded_config(loaded: &LoadedBoltV3Config) -> Result<Self> {
        let path = decision_evidence_path(loaded)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create decision evidence directory `{}`",
                    parent.display()
                )
            })?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| {
                format!("failed to open decision evidence file `{}`", path.display())
            })?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }

    fn append_line(&self, line: &[u8]) -> Result<()> {
        let mut file = self
            .file
            .lock()
            .map_err(|_| anyhow!("decision evidence writer lock is poisoned"))?;
        file.write_all(line)
            .context("failed to write decision evidence record")?;
        file.sync_data()
            .context("failed to sync decision evidence to disk")?;
        Ok(())
    }
}

impl BoltV3DecisionEvidenceWriter for JsonlBoltV3DecisionEvidenceWriter {
    fn record_strategy_input_snapshot(
        &self,
        snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()> {
        let line = encode_strategy_input_snapshot_line(snapshot)?;
        self.append_line(&line)
    }

    fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        let line = encode_order_intent_line(intent)?;
        self.append_line(&line)
    }

    fn record_admission_decision(&self, decision: &BoltV3AdmissionDecisionEvidence) -> Result<()> {
        let line = encode_admission_decision_line(decision)?;
        self.append_line(&line)
    }
}

pub fn decision_evidence_path(loaded: &LoadedBoltV3Config) -> Result<PathBuf> {
    let relative = Path::new(
        loaded
            .root
            .persistence
            .decision_evidence
            .order_intents_relative_path
            .trim(),
    );
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(anyhow!(
            "persistence.decision_evidence.order_intents_relative_path must be non-empty, relative, and stay under catalog_directory"
        ));
    }
    Ok(Path::new(&loaded.root.persistence.catalog_directory).join(relative))
}

#[derive(Serialize)]
struct OrderIntentLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    intent: &'a BoltV3OrderIntentEvidence,
}

#[derive(Serialize)]
struct StrategyInputSnapshotLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    snapshot: &'a BoltV3StrategyInputEvidenceSnapshot,
}

#[derive(Serialize)]
struct AdmissionDecisionLine<'a> {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: &'static str,
    gate_version: &'static str,
    kind: &'static str,
    decision: &'a BoltV3AdmissionDecisionEvidence,
}

fn current_utc_ns() -> i64 {
    chrono::Utc::now()
        .timestamp_nanos_opt()
        .expect("UTC timestamp must fit in i64 nanoseconds")
}

fn encode_order_intent_line(intent: &BoltV3OrderIntentEvidence) -> Result<Vec<u8>> {
    let envelope = OrderIntentLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_ORDER_INTENT_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: "order_intent",
        intent,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize order intent decision evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_strategy_input_snapshot_line(
    snapshot: &BoltV3StrategyInputEvidenceSnapshot,
) -> Result<Vec<u8>> {
    let envelope = StrategyInputSnapshotLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: "strategy_input_snapshot",
        snapshot,
    };
    let mut line = serde_json::to_vec(&envelope)
        .context("failed to serialize strategy input evidence snapshot")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

fn encode_admission_decision_line(decision: &BoltV3AdmissionDecisionEvidence) -> Result<Vec<u8>> {
    let envelope = AdmissionDecisionLine {
        schema_version: BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION,
        recorded_at_utc_ns: current_utc_ns(),
        gate_id: BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
        gate_version: BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
        kind: "admission_decision",
        decision,
    };
    let mut line =
        serde_json::to_vec(&envelope).context("failed to serialize admission decision evidence")?;
    line.extend_from_slice(b"\n");
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_line(line: &[u8]) -> serde_json::Value {
        assert!(line.ends_with(b"\n"), "line must end with newline");
        let json = std::str::from_utf8(&line[..line.len() - 1]).expect("line is utf8");
        serde_json::from_str(json).expect("line is json")
    }

    #[test]
    fn encode_order_intent_line_wraps_intent_with_full_gate_metadata() {
        let intent = BoltV3OrderIntentEvidence {
            strategy_id: "strategy-one".to_string(),
            intent_kind: BoltV3OrderIntentKind::Entry,
            instrument_id: "instrument-one".to_string(),
            client_order_id: "client-order-one".to_string(),
            order_side: "Buy".to_string(),
            price: "0.42".to_string(),
            quantity: "1".to_string(),
        };

        let line = encode_order_intent_line(&intent).expect("intent should encode");
        let decoded = parse_line(&line);

        assert_eq!(
            decoded["schema_version"],
            BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
        );
        assert_eq!(decoded["gate_id"], BOLT_V3_ORDER_INTENT_GATE_ID);
        assert_eq!(
            decoded["gate_version"],
            BOLT_V3_DECISION_EVIDENCE_GATE_VERSION
        );
        assert_eq!(decoded["kind"], "order_intent");
        assert!(
            decoded["recorded_at_utc_ns"]
                .as_i64()
                .map(|ns| ns > 0)
                .unwrap_or(false),
            "recorded_at_utc_ns must be a positive i64; got {:?}",
            decoded["recorded_at_utc_ns"]
        );
        let intent = &decoded["intent"];
        assert_eq!(intent["strategy_id"], "strategy-one");
        assert_eq!(intent["intent_kind"], "entry");
        assert_eq!(intent["order_side"], "Buy");
    }

    #[test]
    fn encode_strategy_input_snapshot_line_wraps_snapshot_with_full_gate_metadata() {
        let snapshot = BoltV3StrategyInputEvidenceSnapshot {
            strategy_id: "strategy-one".to_string(),
            configured_target_id: "target-one".to_string(),
            market_selection_ruleset_id: "target-one".to_string(),
            market_selection_outcome: "current".to_string(),
            market_id: Some("market-one".to_string()),
            up_instrument_id: Some("instrument-up".to_string()),
            down_instrument_id: Some("instrument-down".to_string()),
            market_selection_timestamp_ms: Some(1000),
            selected_market_observed_timestamp_ms: Some(1000),
            polymarket_market_start_timestamp_ms: Some(1000),
            polymarket_market_end_timestamp_ms: Some(301000),
            price_to_beat_source: "source-one".to_string(),
            price_to_beat_value: "3100".to_string(),
            reference_quote_ts_event: 1200,
            spot_price: "3100.5".to_string(),
            reference_fair_value: Some("3100.5".to_string()),
            realized_volatility: "1.5".to_string(),
            seconds_to_market_end: 300,
            pricing_kurtosis: "0".to_string(),
            theta_decay_factor: "0".to_string(),
            theta_scaled_min_edge_bps: "1".to_string(),
            fair_probability_up: "0.6".to_string(),
            uncertainty_band_probability: "0.01".to_string(),
            expected_edge_basis_points: "10".to_string(),
            worst_case_edge_basis_points: "10".to_string(),
            fee_rate_basis_points: "0".to_string(),
            selected_side: Some("up".to_string()),
            submission_instrument_id: "instrument-up".to_string(),
            submission_order_side: "Buy".to_string(),
            submission_price: "0.50".to_string(),
            submission_quantity: "1".to_string(),
            client_order_id: "client-order-one".to_string(),
        };

        let line = encode_strategy_input_snapshot_line(&snapshot).expect("snapshot should encode");
        let decoded = parse_line(&line);

        assert_eq!(
            decoded["schema_version"],
            BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
        );
        assert_eq!(decoded["gate_id"], BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID);
        assert_eq!(
            decoded["gate_version"],
            BOLT_V3_DECISION_EVIDENCE_GATE_VERSION
        );
        assert_eq!(decoded["kind"], "strategy_input_snapshot");
        assert!(
            decoded["recorded_at_utc_ns"]
                .as_i64()
                .map(|ns| ns > 0)
                .unwrap_or(false),
            "recorded_at_utc_ns must be a positive i64; got {:?}",
            decoded["recorded_at_utc_ns"]
        );
        let snapshot_field = &decoded["snapshot"];
        assert_eq!(snapshot_field["strategy_id"], "strategy-one");
        assert_eq!(snapshot_field["price_to_beat_source"], "source-one");
        assert_eq!(snapshot_field["reference_quote_ts_event"], 1200);
        assert_eq!(snapshot_field["client_order_id"], "client-order-one");
    }

    #[test]
    fn encode_admission_decision_line_wraps_decision_with_full_gate_metadata() {
        for outcome in [
            BoltV3AdmissionOutcome::Admitted,
            BoltV3AdmissionOutcome::RejectedNotArmed,
            BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed,
            BoltV3AdmissionOutcome::RejectedNonPositiveNotional,
            BoltV3AdmissionOutcome::RejectedNotionalCapExceeded,
            BoltV3AdmissionOutcome::RejectedCountCapExhausted,
        ] {
            let decision = BoltV3AdmissionDecisionEvidence {
                strategy_id: "strategy-one".to_string(),
                client_order_id: "client-order-one".to_string(),
                instrument_id: "instrument-one".to_string(),
                notional: "1.0".to_string(),
                intent_kind: BoltV3SubmitIntentKind::Entry,
                outcome: outcome.clone(),
            };

            let line = encode_admission_decision_line(&decision).expect("decision should encode");
            let decoded = parse_line(&line);

            assert_eq!(
                decoded["schema_version"],
                BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION
            );
            assert_eq!(decoded["gate_id"], BOLT_V3_SUBMIT_ADMISSION_GATE_ID);
            assert_eq!(
                decoded["gate_version"],
                BOLT_V3_DECISION_EVIDENCE_GATE_VERSION
            );
            assert_eq!(decoded["kind"], "admission_decision");
            assert!(
                decoded["recorded_at_utc_ns"]
                    .as_i64()
                    .map(|ns| ns > 0)
                    .unwrap_or(false),
                "recorded_at_utc_ns must be a positive i64; got {:?}",
                decoded["recorded_at_utc_ns"]
            );
            let decision_field = &decoded["decision"];
            assert_eq!(decision_field["strategy_id"], "strategy-one");
            assert_eq!(decision_field["notional"], "1.0");
            assert_eq!(decision_field["intent_kind"], "entry");
            let expected_outcome = match outcome {
                BoltV3AdmissionOutcome::Admitted => "admitted",
                BoltV3AdmissionOutcome::RejectedNotArmed => "rejected_not_armed",
                BoltV3AdmissionOutcome::RejectedSubmitLifecycleDisallowed => {
                    "rejected_submit_lifecycle_disallowed"
                }
                BoltV3AdmissionOutcome::RejectedNonPositiveNotional => {
                    "rejected_non_positive_notional"
                }
                BoltV3AdmissionOutcome::RejectedNotionalCapExceeded => {
                    "rejected_notional_cap_exceeded"
                }
                BoltV3AdmissionOutcome::RejectedCountCapExhausted => "rejected_count_cap_exhausted",
            };
            assert_eq!(decision_field["outcome"], expected_outcome);
        }
    }

    #[test]
    fn gate_version_constant_matches_package_version() {
        assert_eq!(
            BOLT_V3_DECISION_EVIDENCE_GATE_VERSION,
            env!("CARGO_PKG_VERSION")
        );
    }
}
