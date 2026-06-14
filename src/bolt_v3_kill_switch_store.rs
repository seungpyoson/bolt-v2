use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_config::{KillSwitchConfigBlock, resolve_root_relative_path},
    bolt_v3_kill_switch::KillSwitchState,
    bolt_v3_operator_artifacts::PRIVATE_ARTIFACT_FILE_MODE,
};

pub const KILL_SWITCH_STORE_SCHEMA_VERSION: u32 = 1;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillSwitchLossProtectionSnapshot {
    pub daily_bucket: Option<u64>,
    pub daily_realized_pnl: Decimal,
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

#[derive(Debug)]
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
        let persisted = PersistedKillSwitchState {
            schema_version: KILL_SWITCH_STORE_SCHEMA_VERSION,
            state: state.clone(),
            loss_protection: loss_protection.map(PersistedKillSwitchLossProtectionSnapshot::from),
        };
        self.write_persisted_state(&persisted)
    }

    pub fn invalidate(&self) -> Result<(), KillSwitchStoreError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| KillSwitchStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let temp_path = self.temp_path();
        write_private_synced_file(&temp_path, b"!")?;
        fs::rename(&temp_path, &self.path).map_err(|source| KillSwitchStoreError::Io {
            path: self.path.clone(),
            source,
        })?;
        sync_parent_dir(&self.path)?;
        Ok(())
    }

    fn write_persisted_state(
        &self,
        persisted: &PersistedKillSwitchState,
    ) -> Result<(), KillSwitchStoreError> {
        let bytes =
            serde_json::to_vec_pretty(&persisted).map_err(KillSwitchStoreError::Serialize)?;
        if bytes.len() as u64 > self.max_state_file_bytes {
            return Err(KillSwitchStoreError::StateTooLarge {
                path: self.path.clone(),
                bytes: bytes.len() as u64,
                max_bytes: self.max_state_file_bytes,
            });
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| KillSwitchStoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let temp_path = self.temp_path();
        write_private_synced_file(&temp_path, &bytes)?;
        fs::rename(&temp_path, &self.path).map_err(|source| KillSwitchStoreError::Io {
            path: self.path.clone(),
            source,
        })?;
        sync_parent_dir(&self.path)?;
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

    fn temp_path(&self) -> PathBuf {
        let mut temp_path = self.path.clone();
        temp_path.set_extension("tmp");
        temp_path
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedKillSwitchState {
    schema_version: u32,
    state: KillSwitchState,
    #[serde(skip_serializing_if = "Option::is_none")]
    loss_protection: Option<PersistedKillSwitchLossProtectionSnapshot>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedKillSwitchLossProtectionSnapshot {
    daily_bucket: Option<u64>,
    daily_realized_pnl: String,
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
        Ok(Self {
            daily_bucket: snapshot.daily_bucket,
            daily_realized_pnl: Decimal::from_str(&snapshot.daily_realized_pnl).map_err(|_| ())?,
            cumulative_position_pnl: restore_pnl_map(snapshot.cumulative_position_pnl)?,
            closed_position_pnl: restore_pnl_map(snapshot.closed_position_pnl)?,
            adjusted_position_pnl: restore_pnl_map(snapshot.adjusted_position_pnl)?,
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
            Self::StateTooLarge { .. } => None,
        }
    }
}

fn write_private_synced_file(path: &Path, bytes: &[u8]) -> Result<(), KillSwitchStoreError> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    configure_private_file_options(&mut options);

    let mut file = options
        .open(path)
        .map_err(|source| KillSwitchStoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(bytes)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|source| KillSwitchStoreError::Io {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(unix)]
fn configure_private_file_options(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(PRIVATE_ARTIFACT_FILE_MODE);
}

#[cfg(not(unix))]
fn configure_private_file_options(_options: &mut OpenOptions) {}

fn sync_parent_dir(path: &Path) -> Result<(), KillSwitchStoreError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|source| KillSwitchStoreError::Io {
            path: parent.to_path_buf(),
            source,
        })
}
