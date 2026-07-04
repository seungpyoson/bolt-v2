use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    str::FromStr,
};

use nautilus_model::types::Currency;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_atomic_io::{AtomicIoError, write_private_atomic_file, write_private_new_file},
    bolt_v3_config::{KillSwitchConfigBlock, resolve_root_relative_path},
    bolt_v3_kill_switch::KillSwitchState,
};

pub const KILL_SWITCH_STORE_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillSwitchRecoveryState {
    Recovered(KillSwitchState),
    FailClosed {
        reason: KillSwitchRecoveryReason,
        state: Option<KillSwitchState>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillSwitchRecoveryReason {
    MissingEvidence,
    MissingLossProtectionSnapshot,
    CorruptEvidence,
    OversizedEvidence,
    UnsupportedSchemaVersion,
    UnresolvedHalt,
}

impl std::fmt::Display for KillSwitchRecoveryReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEvidence => write!(f, "missing evidence"),
            Self::MissingLossProtectionSnapshot => write!(f, "missing loss protection snapshot"),
            Self::CorruptEvidence => write!(f, "corrupt evidence"),
            Self::OversizedEvidence => write!(f, "oversized evidence"),
            Self::UnsupportedSchemaVersion => write!(f, "unsupported schema version"),
            Self::UnresolvedHalt => write!(f, "unresolved halt"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillSwitchRecoveryRecord {
    pub recovery_state: KillSwitchRecoveryState,
    pub loss_protection: Option<KillSwitchLossProtectionSnapshot>,
    pub loss_governor_manual_recoveries: Vec<KillSwitchLossGovernorManualRecoveryRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillSwitchLossGovernorManualRecoveryRecord {
    pub operator_id: String,
    pub evidence_path: String,
    pub evidence_sha256: String,
    pub observed_at_ns: u64,
    pub recorded_at_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillSwitchLossProtectionSnapshot {
    pub daily_bucket: Option<u64>,
    pub daily_realized_pnl: Decimal,
    pub settlement_currency: Option<String>,
    pub cumulative_position_pnl: BTreeMap<String, KillSwitchCumulativePositionPnlSnapshot>,
    pub closed_position_pnl: BTreeMap<String, KillSwitchCumulativePositionPnlSnapshot>,
    pub adjusted_position_pnl: BTreeMap<String, KillSwitchCumulativePositionPnlSnapshot>,
    pub pending_halt_actions: Option<KillSwitchPendingHaltActionsSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillSwitchCumulativePositionPnlSnapshot {
    pub realized_pnl: Decimal,
    pub last_observed_at_unix_nanos: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillSwitchPendingHaltActionsSnapshot {
    pub next_retry_at_unix_nanos: u64,
    pub retry_deadline_unix_nanos: u64,
}

#[derive(Debug, Clone)]
pub struct KillSwitchStore {
    path: PathBuf,
    max_state_file_bytes: u64,
}

impl KillSwitchStore {
    pub fn new(path: impl Into<PathBuf>, max_state_file_bytes: u64) -> Self {
        Self {
            path: path.into(),
            max_state_file_bytes,
        }
    }

    pub fn from_root_config_path(root_path: &Path, config: &KillSwitchConfigBlock) -> Self {
        Self::new(
            resolve_root_relative_path(root_path, &config.state_path),
            config.max_state_file_bytes,
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_state(&self, state: &KillSwitchState) -> Result<(), KillSwitchStoreError> {
        self.write_state_with_loss_snapshot(state, None)
    }

    pub fn write_state_with_loss_snapshot(
        &self,
        state: &KillSwitchState,
        loss_protection: Option<&KillSwitchLossProtectionSnapshot>,
    ) -> Result<(), KillSwitchStoreError> {
        let manual_recoveries = self.existing_loss_governor_manual_recoveries();
        let bytes = serialize_state_with_loss_snapshot_and_manual_recoveries(
            state,
            loss_protection,
            &manual_recoveries,
        )?;
        self.ensure_state_bytes_within_limit(&bytes)?;
        write_private_atomic_file(&self.path, &bytes)?;
        Ok(())
    }

    fn existing_loss_governor_manual_recoveries(
        &self,
    ) -> Vec<KillSwitchLossGovernorManualRecoveryRecord> {
        match self.load_recovery_record() {
            Ok(record) => record.loss_governor_manual_recoveries,
            Err(error) => {
                let manual_recoveries = Vec::new();
                log::error!(
                    "failed to preserve loss-governor manual recovery audit history from {}; writing state with loss_governor_manual_recoveries={manual_recoveries:?}: {error}",
                    self.path.display()
                );
                manual_recoveries
            }
        }
    }

    pub fn write_loss_governor_manual_recovery(
        &self,
        previous_record: &KillSwitchRecoveryRecord,
        state: &KillSwitchState,
        loss_protection: &KillSwitchLossProtectionSnapshot,
        manual_recovery: KillSwitchLossGovernorManualRecoveryRecord,
    ) -> Result<usize, KillSwitchStoreError> {
        let mut manual_recoveries = previous_record.loss_governor_manual_recoveries.clone();
        manual_recoveries.push(manual_recovery);
        let bytes = serialize_state_with_loss_snapshot_and_manual_recoveries(
            state,
            Some(loss_protection),
            &manual_recoveries,
        )?;
        self.ensure_state_bytes_within_limit(&bytes)?;
        write_private_atomic_file(&self.path, &bytes)?;
        Ok(manual_recoveries.len())
    }

    pub fn bootstrap_initial_armed_loss_snapshot(&self) -> Result<(), KillSwitchStoreError> {
        let snapshot = initial_armed_loss_protection_snapshot();
        let bytes = serialize_state_with_loss_snapshot(&KillSwitchState::Armed, Some(&snapshot))?;
        self.ensure_state_bytes_within_limit(&bytes)?;
        match write_private_new_file(&self.path, &bytes) {
            Ok(()) => Ok(()),
            Err(error) => {
                if error.source.kind() == io::ErrorKind::AlreadyExists && error.path == self.path {
                    Err(KillSwitchStoreError::StateAlreadyExists { path: error.path })
                } else {
                    Err(KillSwitchStoreError::from(error))
                }
            }
        }
    }

    fn ensure_state_bytes_within_limit(&self, bytes: &[u8]) -> Result<(), KillSwitchStoreError> {
        if bytes.len() as u64 > self.max_state_file_bytes {
            return Err(KillSwitchStoreError::StateTooLarge {
                path: self.path.clone(),
                bytes: bytes.len() as u64,
                max_bytes: self.max_state_file_bytes,
            });
        }
        Ok(())
    }

    pub fn invalidate(&self) -> Result<(), KillSwitchStoreError> {
        write_private_atomic_file(&self.path, b"!")?;
        Ok(())
    }

    pub fn load_recovery_state(&self) -> Result<KillSwitchRecoveryState, KillSwitchStoreError> {
        self.load_recovery_record()
            .map(|record| record.recovery_state)
    }

    pub fn load_recovery_record(&self) -> Result<KillSwitchRecoveryRecord, KillSwitchStoreError> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(KillSwitchRecoveryRecord {
                    recovery_state: KillSwitchRecoveryState::FailClosed {
                        reason: KillSwitchRecoveryReason::MissingEvidence,
                        state: None,
                    },
                    loss_protection: None,
                    loss_governor_manual_recoveries: Vec::new(),
                });
            }
            Err(source) => {
                return Err(KillSwitchStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        let mut bytes = Vec::new();
        let read_limit = self.max_state_file_bytes.saturating_add(1);
        match file.take(read_limit).read_to_end(&mut bytes) {
            Ok(_) => {}
            Err(source) => {
                return Err(KillSwitchStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };
        if bytes.len() as u64 > self.max_state_file_bytes {
            return Ok(KillSwitchRecoveryRecord {
                recovery_state: KillSwitchRecoveryState::FailClosed {
                    reason: KillSwitchRecoveryReason::OversizedEvidence,
                    state: None,
                },
                loss_protection: None,
                loss_governor_manual_recoveries: Vec::new(),
            });
        }

        let persisted = match serde_json::from_slice::<PersistedKillSwitchState>(&bytes) {
            Ok(persisted) => persisted,
            Err(_) => {
                return Ok(KillSwitchRecoveryRecord {
                    recovery_state: KillSwitchRecoveryState::FailClosed {
                        reason: KillSwitchRecoveryReason::CorruptEvidence,
                        state: None,
                    },
                    loss_protection: None,
                    loss_governor_manual_recoveries: Vec::new(),
                });
            }
        };

        let loss_governor_manual_recoveries = persisted
            .loss_governor_manual_recoveries
            .as_deref()
            .unwrap_or(&[])
            .iter()
            .map(KillSwitchLossGovernorManualRecoveryRecord::from)
            .collect::<Vec<_>>();

        if persisted.schema_version != KILL_SWITCH_STORE_SCHEMA_VERSION {
            return Ok(KillSwitchRecoveryRecord {
                recovery_state: KillSwitchRecoveryState::FailClosed {
                    reason: KillSwitchRecoveryReason::UnsupportedSchemaVersion,
                    state: Some(persisted.state),
                },
                loss_protection: None,
                loss_governor_manual_recoveries,
            });
        }

        let loss_protection = match persisted.loss_protection {
            Some(snapshot) => match KillSwitchLossProtectionSnapshot::try_from(snapshot) {
                Ok(snapshot) => Some(snapshot),
                Err(()) => {
                    return Ok(KillSwitchRecoveryRecord {
                        recovery_state: KillSwitchRecoveryState::FailClosed {
                            reason: KillSwitchRecoveryReason::CorruptEvidence,
                            state: Some(persisted.state),
                        },
                        loss_protection: None,
                        loss_governor_manual_recoveries,
                    });
                }
            },
            None => None,
        };

        let recovery_state = match persisted.state {
            KillSwitchState::Halting { .. } | KillSwitchState::FailedManualIntervention { .. } => {
                KillSwitchRecoveryState::FailClosed {
                    reason: KillSwitchRecoveryReason::UnresolvedHalt,
                    state: Some(persisted.state),
                }
            }
            state => KillSwitchRecoveryState::Recovered(state),
        };
        Ok(KillSwitchRecoveryRecord {
            recovery_state,
            loss_protection,
            loss_governor_manual_recoveries,
        })
    }
}

pub fn initial_armed_loss_protection_snapshot() -> KillSwitchLossProtectionSnapshot {
    KillSwitchLossProtectionSnapshot {
        daily_bucket: None,
        daily_realized_pnl: Decimal::ZERO,
        settlement_currency: None,
        cumulative_position_pnl: BTreeMap::new(),
        closed_position_pnl: BTreeMap::new(),
        adjusted_position_pnl: BTreeMap::new(),
        pending_halt_actions: None,
    }
}

fn serialize_state_with_loss_snapshot(
    state: &KillSwitchState,
    loss_protection: Option<&KillSwitchLossProtectionSnapshot>,
) -> Result<Vec<u8>, KillSwitchStoreError> {
    serialize_state_with_loss_snapshot_and_manual_recoveries(state, loss_protection, &[])
}

fn serialize_state_with_loss_snapshot_and_manual_recoveries(
    state: &KillSwitchState,
    loss_protection: Option<&KillSwitchLossProtectionSnapshot>,
    loss_governor_manual_recoveries: &[KillSwitchLossGovernorManualRecoveryRecord],
) -> Result<Vec<u8>, KillSwitchStoreError> {
    let persisted = PersistedKillSwitchState {
        schema_version: KILL_SWITCH_STORE_SCHEMA_VERSION,
        state: state.clone(),
        loss_protection: loss_protection.map(PersistedKillSwitchLossProtectionSnapshot::from),
        loss_governor_manual_recoveries: (!loss_governor_manual_recoveries.is_empty()).then(|| {
            loss_governor_manual_recoveries
                .iter()
                .map(PersistedLossGovernorManualRecoveryRecord::from)
                .collect()
        }),
    };
    let mut bytes =
        serde_json::to_vec_pretty(&persisted).map_err(KillSwitchStoreError::Serialize)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedKillSwitchState {
    schema_version: u32,
    state: KillSwitchState,
    #[serde(skip_serializing_if = "Option::is_none")]
    loss_protection: Option<PersistedKillSwitchLossProtectionSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    loss_governor_manual_recoveries: Option<Vec<PersistedLossGovernorManualRecoveryRecord>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedLossGovernorManualRecoveryRecord {
    operator_id: String,
    evidence_path: String,
    evidence_sha256: String,
    observed_at_ns: u64,
    recorded_at_ns: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedKillSwitchLossProtectionSnapshot {
    daily_bucket: Option<u64>,
    daily_realized_pnl: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    settlement_currency: Option<String>,
    cumulative_position_pnl: BTreeMap<String, PersistedCumulativePositionPnlSnapshot>,
    closed_position_pnl: BTreeMap<String, PersistedCumulativePositionPnlSnapshot>,
    adjusted_position_pnl: BTreeMap<String, PersistedCumulativePositionPnlSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_halt_actions: Option<KillSwitchPendingHaltActionsSnapshot>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedCumulativePositionPnlSnapshot {
    realized_pnl: String,
    last_observed_at_unix_nanos: u64,
}

fn persist_pnl_map(
    map: &BTreeMap<String, KillSwitchCumulativePositionPnlSnapshot>,
) -> BTreeMap<String, PersistedCumulativePositionPnlSnapshot> {
    map.iter()
        .map(|(position_id, value)| {
            (
                position_id.clone(),
                PersistedCumulativePositionPnlSnapshot {
                    realized_pnl: value.realized_pnl.to_string(),
                    last_observed_at_unix_nanos: value.last_observed_at_unix_nanos,
                },
            )
        })
        .collect()
}

fn restore_pnl_map(
    map: BTreeMap<String, PersistedCumulativePositionPnlSnapshot>,
) -> Result<BTreeMap<String, KillSwitchCumulativePositionPnlSnapshot>, ()> {
    let mut restored = BTreeMap::new();
    for (position_id, value) in map {
        restored.insert(
            position_id,
            KillSwitchCumulativePositionPnlSnapshot {
                realized_pnl: Decimal::from_str(&value.realized_pnl).map_err(|_| ())?,
                last_observed_at_unix_nanos: value.last_observed_at_unix_nanos,
            },
        );
    }
    Ok(restored)
}

impl From<&KillSwitchLossProtectionSnapshot> for PersistedKillSwitchLossProtectionSnapshot {
    fn from(snapshot: &KillSwitchLossProtectionSnapshot) -> Self {
        Self {
            daily_bucket: snapshot.daily_bucket,
            daily_realized_pnl: snapshot.daily_realized_pnl.to_string(),
            settlement_currency: snapshot.settlement_currency.clone(),
            cumulative_position_pnl: persist_pnl_map(&snapshot.cumulative_position_pnl),
            closed_position_pnl: persist_pnl_map(&snapshot.closed_position_pnl),
            adjusted_position_pnl: persist_pnl_map(&snapshot.adjusted_position_pnl),
            pending_halt_actions: snapshot.pending_halt_actions,
        }
    }
}

impl From<&KillSwitchLossGovernorManualRecoveryRecord>
    for PersistedLossGovernorManualRecoveryRecord
{
    fn from(record: &KillSwitchLossGovernorManualRecoveryRecord) -> Self {
        Self {
            operator_id: record.operator_id.clone(),
            evidence_path: record.evidence_path.clone(),
            evidence_sha256: record.evidence_sha256.clone(),
            observed_at_ns: record.observed_at_ns,
            recorded_at_ns: record.recorded_at_ns,
        }
    }
}

impl From<&PersistedLossGovernorManualRecoveryRecord>
    for KillSwitchLossGovernorManualRecoveryRecord
{
    fn from(record: &PersistedLossGovernorManualRecoveryRecord) -> Self {
        Self {
            operator_id: record.operator_id.clone(),
            evidence_path: record.evidence_path.clone(),
            evidence_sha256: record.evidence_sha256.clone(),
            observed_at_ns: record.observed_at_ns,
            recorded_at_ns: record.recorded_at_ns,
        }
    }
}

impl TryFrom<PersistedKillSwitchLossProtectionSnapshot> for KillSwitchLossProtectionSnapshot {
    type Error = ();

    fn try_from(snapshot: PersistedKillSwitchLossProtectionSnapshot) -> Result<Self, Self::Error> {
        let daily_realized_pnl = Decimal::from_str(&snapshot.daily_realized_pnl).map_err(|_| ())?;
        let cumulative_position_pnl = restore_pnl_map(snapshot.cumulative_position_pnl)?;
        let closed_position_pnl = restore_pnl_map(snapshot.closed_position_pnl)?;
        if cumulative_position_pnl
            .keys()
            .any(|position_id| closed_position_pnl.contains_key(position_id))
        {
            return Err(());
        }
        let adjusted_position_pnl = restore_pnl_map(snapshot.adjusted_position_pnl)?;
        let has_loss_evidence = daily_realized_pnl != Decimal::ZERO
            || !cumulative_position_pnl.is_empty()
            || !closed_position_pnl.is_empty()
            || !adjusted_position_pnl.is_empty();
        let settlement_currency = match snapshot.settlement_currency {
            Some(currency) if currency.trim().is_empty() || currency.trim() != currency => {
                return Err(());
            }
            Some(currency) if Currency::try_from_str(&currency).is_none() => {
                return Err(());
            }
            Some(currency) => Some(currency),
            None if has_loss_evidence => return Err(()),
            None => None,
        };
        Ok(Self {
            daily_bucket: snapshot.daily_bucket,
            daily_realized_pnl,
            settlement_currency,
            cumulative_position_pnl,
            closed_position_pnl,
            adjusted_position_pnl,
            pending_halt_actions: snapshot.pending_halt_actions,
        })
    }
}

#[derive(Debug)]
pub enum KillSwitchStoreError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Serialize(serde_json::Error),
    StateAlreadyExists {
        path: PathBuf,
    },
    StateTooLarge {
        path: PathBuf,
        bytes: u64,
        max_bytes: u64,
    },
}

impl std::fmt::Display for KillSwitchStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            Self::Serialize(error) => write!(f, "failed to serialize kill-switch state: {error}"),
            Self::StateAlreadyExists { path } => {
                write!(
                    f,
                    "kill-switch state file {} already exists; refusing to bootstrap over existing evidence",
                    path.display()
                )
            }
            Self::StateTooLarge {
                path,
                bytes,
                max_bytes,
            } => write!(
                f,
                "kill-switch state file {} is {bytes} bytes, exceeding the {max_bytes} byte limit",
                path.display()
            ),
        }
    }
}

impl std::error::Error for KillSwitchStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Serialize(error) => Some(error),
            Self::StateAlreadyExists { .. } => None,
            Self::StateTooLarge { .. } => None,
        }
    }
}

impl From<AtomicIoError> for KillSwitchStoreError {
    fn from(error: AtomicIoError) -> Self {
        Self::Io {
            path: error.path,
            source: error.source,
        }
    }
}
