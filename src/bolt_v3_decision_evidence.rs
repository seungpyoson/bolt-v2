use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, anyhow};
use nautilus_model::orders::{Order, OrderAny};
use serde::{Deserialize, Serialize};

use crate::bolt_v3_config::LoadedBoltV3Config;

pub const BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION: u32 = 5;
pub const BOLT_V3_DECISION_EVIDENCE_GATE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BOLT_V3_ORDER_INTENT_GATE_ID: &str = "bolt_v3.order_intent";
pub const BOLT_V3_SUBMIT_ADMISSION_GATE_ID: &str = "bolt_v3.submit_admission";
pub const BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID: &str = "bolt_v3.strategy_input_snapshot";
pub const BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_CURRENT: &str = "current";
pub const BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_NEXT: &str = "next";
const GATE_SATISFACTION_KIND_EVIDENCE: &str = "evidence";
const GATE_SATISFACTION_KIND_NO_RESOLUTION: &str = "no_resolution";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3ReadinessGateEvidenceSnapshot {
    pub gate_session_hash: String,
    pub selected_market_key: String,
    pub gate_evidence: BTreeMap<String, BoltV3GateEvidenceIdentity>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoltV3RuntimeReadinessSeed {
    pub strategy_instance_id: String,
    pub gate_session_hash: String,
    pub selected_market_key: String,
    pub polymarket_condition_id: String,
    pub polymarket_market_slug: String,
    pub polymarket_question_id: String,
    pub up_instrument_id: String,
    pub down_instrument_id: String,
    pub market_start_timestamp_ms: u64,
    pub market_end_timestamp_ms: u64,
    pub price_to_beat_value: f64,
    pub reference_venue: String,
    pub reference_price: f64,
    pub reference_quote_ts_event: u64,
    pub realized_volatility: f64,
}

impl BoltV3ReadinessGateEvidenceSnapshot {
    pub fn from_entry_readiness_gate_session(
        session: &crate::bolt_v3_operator_artifacts::EntryReadinessGateSession,
    ) -> Self {
        let gate_evidence = session
            .satisfied_roles
            .iter()
            .map(|(role, satisfaction)| {
                (
                    role.clone(),
                    BoltV3GateEvidenceIdentity::from_gate_satisfaction(satisfaction),
                )
            })
            .collect();

        Self {
            gate_session_hash: session.session_hash.clone(),
            selected_market_key: session.selected_market.selected_market_key.clone(),
            gate_evidence,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoltV3GateEvidenceIdentity {
    pub satisfaction_kind: String,
    pub selected_market_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub normalized_value_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_provenance_sha256: Option<String>,
    pub artifact_sha256s: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution_identity: Option<String>,
}

impl BoltV3GateEvidenceIdentity {
    fn from_gate_satisfaction(
        satisfaction: &crate::bolt_v3_operator_artifacts::GateSatisfaction,
    ) -> Self {
        match satisfaction {
            crate::bolt_v3_operator_artifacts::GateSatisfaction::Evidence { evidence } => Self {
                satisfaction_kind: GATE_SATISFACTION_KIND_EVIDENCE.to_string(),
                selected_market_key: evidence.selected_market_key.clone(),
                provider_id: Some(evidence.provider_id.clone()),
                provider_kind: Some(evidence.provider_kind.clone()),
                value_kind: Some(evidence.value_kind.clone()),
                normalized_value_sha256: Some(evidence.normalized_value_sha256.clone()),
                provider_provenance_sha256: Some(evidence.provider_provenance_sha256.clone()),
                artifact_sha256s: evidence
                    .artifact_refs
                    .iter()
                    .map(|artifact| artifact.sha256.clone())
                    .collect(),
                resolution_identity: None,
            },
            crate::bolt_v3_operator_artifacts::GateSatisfaction::NoResolution {
                selected_market_key,
                resolution_identity,
            } => Self {
                satisfaction_kind: GATE_SATISFACTION_KIND_NO_RESOLUTION.to_string(),
                selected_market_key: selected_market_key.clone(),
                provider_id: None,
                provider_kind: None,
                value_kind: None,
                normalized_value_sha256: None,
                provider_provenance_sha256: None,
                artifact_sha256s: Vec::new(),
                resolution_identity: Some(resolution_identity.clone()),
            },
        }
    }
}

pub trait BoltV3DecisionEvidenceWriter: std::fmt::Debug + Send + Sync {
    fn record_strategy_input_snapshot(
        &self,
        snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()>;

    fn record_order_intent(&self, intent: &BoltV3OrderIntentEvidence) -> Result<()>;
    fn record_admission_decision(&self, decision: &BoltV3AdmissionDecisionEvidence) -> Result<()>;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canary_proof_claim: Option<String>,
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
            canary_proof_claim: None,
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
pub struct BoltV3StrategyInputEvidenceSnapshot {
    pub strategy_id: String,
    pub configured_target_id: String,
    pub market_selection_ruleset_id: String,
    pub gate_session_hash: String,
    pub selected_market_key: String,
    pub gate_evidence: BTreeMap<String, BoltV3GateEvidenceIdentity>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoltV3AdmissionOutcome {
    Admitted,
    RejectedNotArmed,
    RejectedSubmitLifecycleDisallowed,
    RejectedNonPositiveNotional,
    RejectedNotionalCapExceeded,
    RejectedInvalidCanaryProofClaim,
    RejectedCountCapExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        let file = open_decision_evidence_append_file(&path).with_context(|| {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3EntryDecisionEvidenceChain {
    pub snapshot: BoltV3StrategyInputEvidenceSnapshot,
    pub intent: BoltV3OrderIntentEvidence,
    pub admission: BoltV3AdmissionDecisionEvidence,
}

pub fn read_latest_entry_decision_evidence_chain(
    path: impl AsRef<Path>,
    max_bytes: u64,
) -> Result<BoltV3EntryDecisionEvidenceChain> {
    let path = path.as_ref();
    let mut file = open_regular_decision_evidence_file(path)
        .context("failed to open regular file bolt-v3 decision evidence")?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .context("failed to read bolt-v3 decision evidence file")?;
    if bytes.len() as u64 > max_bytes {
        return Err(anyhow!(
            "bolt-v3 decision evidence file exceeds max_bytes={max_bytes}"
        ));
    }

    let mut snapshots = BTreeMap::<String, BoltV3StrategyInputEvidenceSnapshot>::new();
    let mut intents = BTreeMap::<String, BoltV3OrderIntentEvidence>::new();
    let mut admissions = BTreeMap::<String, BoltV3AdmissionDecisionEvidence>::new();
    let mut latest = None;
    for (index, line) in bytes.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let header: DecisionEvidenceEnvelopeHeader =
            serde_json::from_slice(line).with_context(|| {
                format!("failed to parse bolt-v3 decision evidence envelope at line index {index}")
            })?;
        match header.kind.as_str() {
            "strategy_input_snapshot" => {
                let decoded: StrategyInputSnapshotLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!(
                            "failed to parse bolt-v3 strategy input snapshot line at index {index}"
                        )
                    })?;
                decoded.validate_header(
                    "strategy_input_snapshot",
                    BOLT_V3_STRATEGY_INPUT_SNAPSHOT_GATE_ID,
                    index,
                )?;
                snapshots.insert(decoded.snapshot.client_order_id.clone(), decoded.snapshot);
            }
            "order_intent" => {
                let decoded: OrderIntentLineOwned =
                    serde_json::from_slice(line).with_context(|| {
                        format!("failed to parse bolt-v3 order intent line at index {index}")
                    })?;
                decoded.validate_header("order_intent", BOLT_V3_ORDER_INTENT_GATE_ID, index)?;
                if decoded.intent.intent_kind == BoltV3OrderIntentKind::Entry {
                    intents.insert(decoded.intent.client_order_id.clone(), decoded.intent);
                }
            }
            "admission_decision" => {
                let decoded: AdmissionDecisionLineOwned = serde_json::from_slice(line)
                    .with_context(|| {
                        format!("failed to parse bolt-v3 admission decision line at index {index}")
                    })?;
                decoded.validate_header(
                    "admission_decision",
                    BOLT_V3_SUBMIT_ADMISSION_GATE_ID,
                    index,
                )?;
                if decoded.decision.intent_kind == BoltV3SubmitIntentKind::Entry {
                    let client_order_id = decoded.decision.client_order_id.clone();
                    admissions.insert(client_order_id.clone(), decoded.decision);
                    if let (Some(snapshot), Some(intent), Some(admission)) = (
                        snapshots.get(&client_order_id),
                        intents.get(&client_order_id),
                        admissions.get(&client_order_id),
                    ) {
                        latest = Some(validate_entry_decision_chain(
                            snapshot.clone(),
                            intent.clone(),
                            admission.clone(),
                        )?);
                    }
                }
            }
            other => {
                return Err(anyhow!(
                    "unsupported bolt-v3 decision evidence kind `{other}` at line index {index}"
                ));
            }
        }
    }
    latest.ok_or_else(|| anyhow!("bolt-v3 decision evidence has no complete entry decision chain"))
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

fn open_decision_evidence_append_file(path: &Path) -> std::io::Result<fs::File> {
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

    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
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

fn validate_entry_decision_chain(
    snapshot: BoltV3StrategyInputEvidenceSnapshot,
    intent: BoltV3OrderIntentEvidence,
    admission: BoltV3AdmissionDecisionEvidence,
) -> Result<BoltV3EntryDecisionEvidenceChain> {
    validate_strategy_input_readiness_evidence(&snapshot)?;
    if snapshot.strategy_id != intent.strategy_id || snapshot.strategy_id != admission.strategy_id {
        return Err(anyhow!(
            "bolt-v3 entry decision evidence strategy_id mismatch"
        ));
    }
    if snapshot.submission_instrument_id != intent.instrument_id
        || snapshot.submission_instrument_id != admission.instrument_id
    {
        return Err(anyhow!(
            "bolt-v3 entry decision evidence instrument_id mismatch"
        ));
    }
    if snapshot.submission_order_side != intent.order_side {
        return Err(anyhow!(
            "bolt-v3 entry decision evidence order_side mismatch"
        ));
    }
    if snapshot.submission_price != intent.price {
        return Err(anyhow!("bolt-v3 entry decision evidence price mismatch"));
    }
    if snapshot.submission_quantity != intent.quantity {
        return Err(anyhow!("bolt-v3 entry decision evidence quantity mismatch"));
    }
    Ok(BoltV3EntryDecisionEvidenceChain {
        snapshot,
        intent,
        admission,
    })
}

pub(crate) fn validate_strategy_input_readiness_evidence(
    snapshot: &BoltV3StrategyInputEvidenceSnapshot,
) -> Result<()> {
    validate_readiness_gate_evidence_snapshot(&BoltV3ReadinessGateEvidenceSnapshot {
        gate_session_hash: snapshot.gate_session_hash.clone(),
        selected_market_key: snapshot.selected_market_key.clone(),
        gate_evidence: snapshot.gate_evidence.clone(),
    })
}

pub(crate) fn validate_readiness_gate_evidence_snapshot(
    snapshot: &BoltV3ReadinessGateEvidenceSnapshot,
) -> Result<()> {
    ensure_non_empty(
        snapshot.gate_session_hash.as_str(),
        "bolt-v3 entry decision evidence gate_session_hash is missing",
    )?;
    ensure_non_empty(
        snapshot.selected_market_key.as_str(),
        "bolt-v3 entry decision evidence selected_market_key is missing",
    )?;
    if snapshot.gate_evidence.is_empty() {
        return Err(anyhow!(
            "bolt-v3 entry decision evidence gate_evidence is missing"
        ));
    }
    for (role, identity) in &snapshot.gate_evidence {
        ensure_non_empty(
            role.as_str(),
            "bolt-v3 entry decision evidence gate_evidence role is missing",
        )?;
        if identity.selected_market_key != snapshot.selected_market_key {
            return Err(anyhow!(
                "bolt-v3 entry decision evidence selected_market_key mismatch for gate_evidence role `{role}`"
            ));
        }
        ensure_non_empty(
            identity.satisfaction_kind.as_str(),
            "bolt-v3 entry decision evidence gate_evidence satisfaction_kind is missing",
        )?;
        match identity.satisfaction_kind.as_str() {
            GATE_SATISFACTION_KIND_EVIDENCE => {
                ensure_option_non_empty(
                    identity.provider_id.as_deref(),
                    "bolt-v3 entry decision evidence gate_evidence provider_id is missing",
                )?;
                ensure_option_non_empty(
                    identity.provider_kind.as_deref(),
                    "bolt-v3 entry decision evidence gate_evidence provider_kind is missing",
                )?;
                ensure_option_non_empty(
                    identity.value_kind.as_deref(),
                    "bolt-v3 entry decision evidence gate_evidence value_kind is missing",
                )?;
                ensure_option_non_empty(
                    identity.normalized_value_sha256.as_deref(),
                    "bolt-v3 entry decision evidence gate_evidence normalized_value_sha256 is missing",
                )?;
                ensure_option_non_empty(
                    identity.provider_provenance_sha256.as_deref(),
                    "bolt-v3 entry decision evidence gate_evidence provider_provenance_sha256 is missing",
                )?;
                if identity.artifact_sha256s.is_empty() {
                    return Err(anyhow!(
                        "bolt-v3 entry decision evidence gate_evidence artifact_sha256s is missing"
                    ));
                }
            }
            GATE_SATISFACTION_KIND_NO_RESOLUTION => {
                ensure_option_non_empty(
                    identity.resolution_identity.as_deref(),
                    "bolt-v3 entry decision evidence gate_evidence resolution_identity is missing",
                )?;
            }
            other => {
                return Err(anyhow!(
                    "bolt-v3 entry decision evidence gate_evidence satisfaction_kind `{other}` is unsupported"
                ));
            }
        }
    }
    Ok(())
}

fn ensure_option_non_empty(value: Option<&str>, message: &'static str) -> Result<()> {
    let Some(value) = value else {
        return Err(anyhow!(message));
    };
    ensure_non_empty(value, message)
}

fn ensure_non_empty(value: &str, message: &'static str) -> Result<()> {
    if value.is_empty() {
        return Err(anyhow!(message));
    }
    Ok(())
}

#[derive(Deserialize)]
struct DecisionEvidenceEnvelopeHeader {
    schema_version: u32,
    recorded_at_utc_ns: i64,
    gate_id: String,
    gate_version: String,
    kind: String,
}

impl DecisionEvidenceEnvelopeHeader {
    fn validate(&self, expected_kind: &str, expected_gate_id: &str, index: usize) -> Result<()> {
        if self.schema_version != BOLT_V3_DECISION_EVIDENCE_SCHEMA_VERSION {
            return Err(anyhow!(
                "bolt-v3 decision evidence schema_version mismatch at line index {index}"
            ));
        }
        if self.recorded_at_utc_ns <= 0 {
            return Err(anyhow!(
                "bolt-v3 decision evidence recorded_at_utc_ns must be positive at line index {index}"
            ));
        }
        if self.gate_id != expected_gate_id {
            return Err(anyhow!(
                "bolt-v3 decision evidence gate_id mismatch at line index {index}"
            ));
        }
        if self.gate_version != BOLT_V3_DECISION_EVIDENCE_GATE_VERSION {
            return Err(anyhow!(
                "bolt-v3 decision evidence gate_version mismatch at line index {index}"
            ));
        }
        if self.kind != expected_kind {
            return Err(anyhow!(
                "bolt-v3 decision evidence kind mismatch at line index {index}"
            ));
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct StrategyInputSnapshotLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    snapshot: BoltV3StrategyInputEvidenceSnapshot,
}

impl StrategyInputSnapshotLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

#[derive(Deserialize)]
struct OrderIntentLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    intent: BoltV3OrderIntentEvidence,
}

impl OrderIntentLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        self.header.validate(expected_kind, expected_gate_id, index)
    }
}

#[derive(Deserialize)]
struct AdmissionDecisionLineOwned {
    #[serde(flatten)]
    header: DecisionEvidenceEnvelopeHeader,
    decision: BoltV3AdmissionDecisionEvidence,
}

impl AdmissionDecisionLineOwned {
    fn validate_header(
        &self,
        expected_kind: &str,
        expected_gate_id: &str,
        index: usize,
    ) -> Result<()> {
        self.header.validate(expected_kind, expected_gate_id, index)
    }
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

    use nautilus_core::{UUID4, UnixNanos};
    use nautilus_model::{
        enums::{OrderSide, OrderType, TimeInForce, TriggerType},
        identifiers::{ClientOrderId, InstrumentId, StrategyId, TraderId},
        orders::StopMarketOrder,
        types::{Price, Quantity},
    };

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
            order_side: OrderSide::Buy.to_string(),
            price: "0.42".to_string(),
            quantity: "1".to_string(),
            canary_proof_claim: None,
            order_fields: BoltV3OrderIntentOrderFields {
                order_type: OrderType::Limit.to_string(),
                time_in_force: TimeInForce::Gtc.to_string(),
                price: Some("0.42".to_string()),
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
        assert_eq!(intent["order_side"], OrderSide::Buy.to_string());
        assert_eq!(
            intent["order_fields"]["order_type"],
            OrderType::Limit.to_string()
        );
        assert_eq!(
            intent["order_fields"]["time_in_force"],
            TimeInForce::Gtc.to_string()
        );
        assert_eq!(intent["order_fields"]["price"], "0.42");
        assert_eq!(
            intent["order_fields"]["trigger_price"],
            serde_json::Value::Null
        );
        assert_eq!(intent["order_fields"]["is_post_only"], true);
        assert_eq!(intent["order_fields"]["is_reduce_only"], false);
        assert_eq!(intent["order_fields"]["is_quote_quantity"], false);
    }

    #[test]
    fn order_intent_from_compiled_order_binds_selected_nt_order_fields() {
        let quantity = Quantity::new(2.0, 2);
        let trigger_price = Price::new(0.52, 2);
        let trigger_instrument_id = InstrumentId::from("trigger-instrument.SIM");
        let order = OrderAny::StopMarket(
            StopMarketOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-one"),
                InstrumentId::from("instrument-one.SIM"),
                ClientOrderId::from("client-order-one"),
                OrderSide::Buy,
                quantity,
                trigger_price,
                TriggerType::LastPrice,
                TimeInForce::Gtc,
                None,
                false,
                false,
                None,
                None,
                Some(trigger_instrument_id),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                UUID4::new(),
                UnixNanos::from(1_u64),
            )
            .expect("stop-market order should be valid"),
        );

        let intent = BoltV3OrderIntentEvidence::from_compiled_order(
            "strategy-one".to_string(),
            BoltV3OrderIntentKind::Entry,
            "0.42".to_string(),
            &order,
        );

        assert_eq!(intent.instrument_id, order.instrument_id().to_string());
        assert_eq!(intent.client_order_id, order.client_order_id().to_string());
        assert_eq!(intent.order_side, order.order_side().to_string());
        assert_eq!(intent.price, trigger_price.to_string());
        assert_eq!(intent.quantity, quantity.to_string());
        assert_eq!(
            intent.order_fields.order_type,
            OrderType::StopMarket.to_string()
        );
        assert_eq!(
            intent.order_fields.time_in_force,
            TimeInForce::Gtc.to_string()
        );
        assert_eq!(intent.order_fields.price, None);
        assert_eq!(
            intent.order_fields.trigger_price,
            Some(trigger_price.to_string())
        );
        assert_eq!(
            intent.order_fields.trigger_type,
            Some(TriggerType::LastPrice.to_string())
        );
        assert_eq!(
            intent.order_fields.trigger_instrument_id,
            Some(trigger_instrument_id.to_string())
        );
        assert!(!intent.order_fields.is_post_only);
        assert!(!intent.order_fields.is_reduce_only);
        assert!(!intent.order_fields.is_quote_quantity);
    }

    #[test]
    fn compiled_order_price_source_prefers_activation_price_before_fallback() {
        let activation_price = Price::new(0.48, 2).to_string();
        let fallback_price = Price::new(0.40, 2).to_string();

        assert_eq!(
            selected_compiled_order_price_source(
                None,
                None,
                Some(activation_price.clone()),
                fallback_price,
            ),
            activation_price
        );
    }

    #[test]
    fn encode_strategy_input_snapshot_line_wraps_snapshot_with_full_gate_metadata() {
        let snapshot = BoltV3StrategyInputEvidenceSnapshot {
            strategy_id: "strategy-one".to_string(),
            configured_target_id: "target-one".to_string(),
            market_selection_ruleset_id: "target-one".to_string(),
            gate_session_hash: "gate-session-hash-one".to_string(),
            selected_market_key: "selected-market-key-one".to_string(),
            gate_evidence: BTreeMap::from([(
                "resolution_price".to_string(),
                BoltV3GateEvidenceIdentity {
                    satisfaction_kind: "evidence".to_string(),
                    selected_market_key: "selected-market-key-one".to_string(),
                    provider_id: Some("provider-one".to_string()),
                    provider_kind: Some("chainlink_data_streams".to_string()),
                    value_kind: Some("price".to_string()),
                    normalized_value_sha256: Some("normalized-value-sha-one".to_string()),
                    provider_provenance_sha256: Some("provider-provenance-sha-one".to_string()),
                    artifact_sha256s: vec!["artifact-sha-one".to_string()],
                    resolution_identity: None,
                },
            )]),
            market_selection_outcome: "current".to_string(),
            market_id: Some("market-one".to_string()),
            polymarket_condition_id: Some("condition-one".to_string()),
            polymarket_market_slug: Some("market-slug-one".to_string()),
            polymarket_question_id: Some("question-one".to_string()),
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
        assert_eq!(snapshot_field["gate_session_hash"], "gate-session-hash-one");
        assert_eq!(
            snapshot_field["selected_market_key"],
            "selected-market-key-one"
        );
        assert_eq!(
            snapshot_field["gate_evidence"]["resolution_price"]["provider_id"],
            "provider-one"
        );
        assert_eq!(
            snapshot_field["gate_evidence"]["resolution_price"]["normalized_value_sha256"],
            "normalized-value-sha-one"
        );
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
                BoltV3AdmissionOutcome::RejectedInvalidCanaryProofClaim => {
                    "rejected_invalid_canary_proof_claim"
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
