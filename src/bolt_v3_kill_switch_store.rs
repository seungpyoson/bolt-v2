use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    io::{self, BufRead, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    str::FromStr,
};

use nautilus_model::types::Currency;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_atomic_io::{
        AtomicIoError, append_private_file, write_private_atomic_file, write_private_new_file,
    },
    bolt_v3_config::{KillSwitchConfigBlock, resolve_root_relative_path},
    bolt_v3_kill_switch::KillSwitchState,
};

pub const KILL_SWITCH_STORE_SCHEMA_VERSION: u32 = 2;
const LOSS_GOVERNOR_MANUAL_RECOVERY_AUDIT_SUFFIX: &str = "-manual-recoveries.jsonl";

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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KillSwitchLossGovernorManualRecoveryRecord {
    pub operator_id: String,
    pub evidence_path: String,
    pub evidence_sha256: String,
    pub observed_at_ns: u64,
    pub recorded_at_ns: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<KillSwitchLossGovernorManualRecoveryOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum KillSwitchLossGovernorManualRecoveryOutcome {
    Attempted,
    Recovered,
    RefusedWithReason,
    WriteFailed,
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

    pub fn loss_governor_manual_recovery_audit_path(&self) -> PathBuf {
        let mut file_name = self
            .path
            .file_stem()
            .map_or_else(|| OsString::from("kill-switch"), OsString::from);
        file_name.push(LOSS_GOVERNOR_MANUAL_RECOVERY_AUDIT_SUFFIX);
        self.path.with_file_name(file_name)
    }

    pub fn write_state(&self, state: &KillSwitchState) -> Result<(), KillSwitchStoreError> {
        self.write_state_with_loss_snapshot(state, None)
    }

    pub fn write_state_with_loss_snapshot(
        &self,
        state: &KillSwitchState,
        loss_protection: Option<&KillSwitchLossProtectionSnapshot>,
    ) -> Result<(), KillSwitchStoreError> {
        let bytes = serialize_state_with_loss_snapshot(state, loss_protection)?;
        self.ensure_state_bytes_within_limit(&bytes)?;
        write_private_atomic_file(&self.path, &bytes)?;
        Ok(())
    }

    pub fn append_loss_governor_manual_recovery(
        &self,
        manual_recovery: KillSwitchLossGovernorManualRecoveryRecord,
    ) -> Result<usize, KillSwitchStoreError> {
        let previous_count = self.loss_governor_manual_recovery_audit_appendable_line_count()?;
        let mut bytes =
            serde_json::to_vec(&manual_recovery).map_err(KillSwitchStoreError::Serialize)?;
        bytes.push(b'\n');
        append_private_file(&self.loss_governor_manual_recovery_audit_path(), &bytes)?;
        Ok(previous_count.saturating_add(1))
    }

    pub fn load_loss_governor_manual_recoveries(
        &self,
    ) -> Result<Vec<KillSwitchLossGovernorManualRecoveryRecord>, KillSwitchStoreError> {
        let audit_path = self.loss_governor_manual_recovery_audit_path();
        let contents = match fs::read_to_string(&audit_path) {
            Ok(contents) => contents,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(KillSwitchStoreError::Io {
                    path: audit_path,
                    source,
                });
            }
        };
        let lines: Vec<&str> = contents.lines().collect();
        let final_line_index = lines.len().saturating_sub(1);
        let mut records = Vec::new();
        for (line_index, line) in lines.into_iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str(line) {
                Ok(record) => records.push(record),
                Err(source) if line_index == final_line_index && !contents.ends_with('\n') => {
                    log::error!(
                        "skipping unparseable final loss-governor manual recovery audit line in {}: {source}",
                        audit_path.display()
                    );
                }
                Err(source) => {
                    return Err(KillSwitchStoreError::Deserialize {
                        path: audit_path.clone(),
                        source,
                    });
                }
            }
        }
        Ok(records)
    }

    fn loss_governor_manual_recovery_audit_appendable_line_count(
        &self,
    ) -> Result<usize, KillSwitchStoreError> {
        let audit_path = self.loss_governor_manual_recovery_audit_path();
        let mut file = match fs::File::open(&audit_path) {
            Ok(file) => file,
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(source) => {
                return Err(KillSwitchStoreError::Io {
                    path: audit_path,
                    source,
                });
            }
        };
        let file_len = file
            .metadata()
            .map_err(|source| KillSwitchStoreError::Io {
                path: audit_path.clone(),
                source,
            })?
            .len();
        if file_len == 0 {
            return Ok(0);
        }
        file.seek(SeekFrom::End(-1))
            .map_err(|source| KillSwitchStoreError::Io {
                path: audit_path.clone(),
                source,
            })?;
        let mut last_byte = [0_u8; 1];
        file.read_exact(&mut last_byte)
            .map_err(|source| KillSwitchStoreError::Io {
                path: audit_path.clone(),
                source,
            })?;
        if last_byte[0] != b'\n' {
            log::error!(
                "refusing to append loss-governor manual recovery audit line because {} does not end with a newline",
                audit_path.display()
            );
            return Err(KillSwitchStoreError::TornManualRecoveryAudit { path: audit_path });
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|source| KillSwitchStoreError::Io {
                path: audit_path.clone(),
                source,
            })?;
        let reader = io::BufReader::new(file);
        let mut count = 0;
        for line in reader.lines() {
            let line = line.map_err(|source| KillSwitchStoreError::Io {
                path: audit_path.clone(),
                source,
            })?;
            if !line.trim().is_empty() {
                count += 1;
            }
        }
        Ok(count)
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
                });
            }
        };

        if persisted.schema_version != KILL_SWITCH_STORE_SCHEMA_VERSION {
            return Ok(KillSwitchRecoveryRecord {
                recovery_state: KillSwitchRecoveryState::FailClosed {
                    reason: KillSwitchRecoveryReason::UnsupportedSchemaVersion,
                    state: Some(persisted.state),
                },
                loss_protection: None,
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
    let persisted = PersistedKillSwitchState {
        schema_version: KILL_SWITCH_STORE_SCHEMA_VERSION,
        state: state.clone(),
        loss_protection: loss_protection.map(PersistedKillSwitchLossProtectionSnapshot::from),
    };
    let mut bytes =
        serde_json::to_vec_pretty(&persisted).map_err(KillSwitchStoreError::Serialize)?;
    bytes.push(b'\n');
    Ok(bytes)
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedKillSwitchState {
    schema_version: u32,
    #[serde(rename = "state_v2")]
    state: KillSwitchState,
    #[serde(skip_serializing_if = "Option::is_none")]
    loss_protection: Option<PersistedKillSwitchLossProtectionSnapshot>,
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
    Deserialize {
        path: PathBuf,
        source: serde_json::Error,
    },
    StateAlreadyExists {
        path: PathBuf,
    },
    StateTooLarge {
        path: PathBuf,
        bytes: u64,
        max_bytes: u64,
    },
    TornManualRecoveryAudit {
        path: PathBuf,
    },
}

impl std::fmt::Display for KillSwitchStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "failed to access {}: {source}", path.display())
            }
            Self::Serialize(error) => write!(f, "failed to serialize kill-switch state: {error}"),
            Self::Deserialize { path, source } => {
                write!(f, "failed to parse {}: {source}", path.display())
            }
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
            Self::TornManualRecoveryAudit { path } => write!(
                f,
                "loss-governor manual recovery audit file {} does not end with a newline; refusing to append onto a torn line",
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
            Self::Deserialize { source, .. } => Some(source),
            Self::StateAlreadyExists { .. } => None,
            Self::StateTooLarge { .. } => None,
            Self::TornManualRecoveryAudit { .. } => None,
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
