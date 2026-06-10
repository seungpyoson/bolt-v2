use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

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
    CorruptEvidence,
    OversizedEvidence,
    UnsupportedSchemaVersion,
    UnresolvedHalt,
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
            resolve_root_relative_path(root_path, &config.store_path),
            config.max_state_file_bytes,
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_state(&self, state: &KillSwitchState) -> Result<(), KillSwitchStoreError> {
        let persisted = PersistedKillSwitchState {
            schema_version: KILL_SWITCH_STORE_SCHEMA_VERSION,
            state: state.clone(),
        };
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
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
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
            return Ok(KillSwitchRecoveryState::FailClosed {
                reason: KillSwitchRecoveryReason::OversizedEvidence,
                state: None,
            });
        }

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
