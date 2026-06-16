use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_atomic_io::{AtomicIoError, write_private_atomic_file},
    bolt_v3_config::{KillSwitchConfigBlock, resolve_root_relative_path},
    bolt_v3_kill_switch::KillSwitchState,
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
    CorruptEvidence,
    UnsupportedSchemaVersion,
    UnresolvedHalt,
}

#[derive(Debug)]
pub struct KillSwitchStore {
    path: PathBuf,
}

impl KillSwitchStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn from_root_config_path(root_path: &Path, config: &KillSwitchConfigBlock) -> Self {
        Self::new(resolve_root_relative_path(root_path, &config.state_path))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_state(&self, state: &KillSwitchState) -> Result<(), KillSwitchStoreError> {
        let persisted = PersistedKillSwitchState {
            schema_version: KILL_SWITCH_STORE_SCHEMA_VERSION,
            state: state.clone(),
        };
        let mut bytes =
            serde_json::to_vec_pretty(&persisted).map_err(KillSwitchStoreError::Serialize)?;
        bytes.push(b'\n');
        write_private_atomic_file(&self.path, &bytes)?;
        Ok(())
    }

    pub fn load_recovery_state(&self) -> Result<KillSwitchRecoveryState, KillSwitchStoreError> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(KillSwitchRecoveryState::FailClosed {
                    reason: KillSwitchRecoveryReason::MissingEvidence,
                    state: None,
                });
            }
            Err(source) => {
                return Err(KillSwitchStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        let persisted = match serde_json::from_slice::<PersistedKillSwitchState>(&bytes) {
            Ok(persisted) => persisted,
            Err(_) => {
                return Ok(KillSwitchRecoveryState::FailClosed {
                    reason: KillSwitchRecoveryReason::CorruptEvidence,
                    state: None,
                });
            }
        };

        if persisted.schema_version != KILL_SWITCH_STORE_SCHEMA_VERSION {
            return Ok(KillSwitchRecoveryState::FailClosed {
                reason: KillSwitchRecoveryReason::UnsupportedSchemaVersion,
                state: Some(persisted.state),
            });
        }

        match persisted.state {
            KillSwitchState::Halting { .. } | KillSwitchState::FailedManualIntervention { .. } => {
                Ok(KillSwitchRecoveryState::FailClosed {
                    reason: KillSwitchRecoveryReason::UnresolvedHalt,
                    state: Some(persisted.state),
                })
            }
            state => Ok(KillSwitchRecoveryState::Recovered(state)),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedKillSwitchState {
    schema_version: u32,
    state: KillSwitchState,
}

#[derive(Debug)]
pub enum KillSwitchStoreError {
    Io { path: PathBuf, source: io::Error },
    Serialize(serde_json::Error),
}

impl From<AtomicIoError> for KillSwitchStoreError {
    fn from(error: AtomicIoError) -> Self {
        Self::Io {
            path: error.path,
            source: error.source,
        }
    }
}
