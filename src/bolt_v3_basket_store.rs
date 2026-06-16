use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    bolt_v3_atomic_io::{AtomicIoError, write_private_atomic_file},
    bolt_v3_basket_execution::{BoltV3BasketExecutionState, BoltV3BasketExecutionStatus},
    bolt_v3_config::resolve_root_relative_path,
    bolt_v3_outcome_group_sources::BasketExecutionRiskBlock,
};

pub const BASKET_STORE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoltV3BasketRecoveryState {
    Recovered(BoltV3BasketExecutionState),
    FailClosed {
        reason: BoltV3BasketRecoveryReason,
        state: Option<BoltV3BasketExecutionState>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoltV3BasketRecoveryReason {
    MissingEvidence,
    CorruptEvidence,
    UnsupportedSchemaVersion,
    StateFileTooLarge,
    UnresolvedStuck,
}

#[derive(Debug)]
pub struct BoltV3BasketStore {
    path: PathBuf,
    max_state_file_bytes: u64,
}

impl BoltV3BasketStore {
    pub fn new(path: impl Into<PathBuf>, max_state_file_bytes: u64) -> Self {
        Self {
            path: path.into(),
            max_state_file_bytes,
        }
    }

    pub fn from_root_config_path(root_path: &Path, config: &BasketExecutionRiskBlock) -> Self {
        Self::new(
            resolve_root_relative_path(root_path, &config.state_path),
            config.max_state_file_bytes,
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn write_state(
        &self,
        state: &BoltV3BasketExecutionState,
    ) -> Result<(), BoltV3BasketStoreError> {
        let persisted = PersistedBasketState {
            schema_version: BASKET_STORE_SCHEMA_VERSION,
            state: state.clone(),
        };
        let mut bytes =
            serde_json::to_vec_pretty(&persisted).map_err(BoltV3BasketStoreError::Serialize)?;
        bytes.push(b'\n');
        if bytes.len() as u64 > self.max_state_file_bytes {
            return Err(BoltV3BasketStoreError::StateFileTooLarge {
                max_state_file_bytes: self.max_state_file_bytes,
                actual_state_file_bytes: bytes.len() as u64,
            });
        }
        write_private_atomic_file(&self.path, &bytes)?;
        Ok(())
    }

    pub fn load_recovery_state(&self) -> Result<BoltV3BasketRecoveryState, BoltV3BasketStoreError> {
        match fs::metadata(&self.path) {
            Ok(metadata) if metadata.len() > self.max_state_file_bytes => {
                return Ok(BoltV3BasketRecoveryState::FailClosed {
                    reason: BoltV3BasketRecoveryReason::StateFileTooLarge,
                    state: None,
                });
            }
            Ok(_) => {}
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(BoltV3BasketRecoveryState::FailClosed {
                    reason: BoltV3BasketRecoveryReason::MissingEvidence,
                    state: None,
                });
            }
            Err(source) => {
                return Err(BoltV3BasketStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        }

        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                return Ok(BoltV3BasketRecoveryState::FailClosed {
                    reason: BoltV3BasketRecoveryReason::MissingEvidence,
                    state: None,
                });
            }
            Err(source) => {
                return Err(BoltV3BasketStoreError::Io {
                    path: self.path.clone(),
                    source,
                });
            }
        };

        if bytes.len() as u64 > self.max_state_file_bytes {
            return Ok(BoltV3BasketRecoveryState::FailClosed {
                reason: BoltV3BasketRecoveryReason::StateFileTooLarge,
                state: None,
            });
        }

        let persisted = match serde_json::from_slice::<PersistedBasketState>(&bytes) {
            Ok(persisted) => persisted,
            Err(_) => {
                return Ok(BoltV3BasketRecoveryState::FailClosed {
                    reason: BoltV3BasketRecoveryReason::CorruptEvidence,
                    state: None,
                });
            }
        };

        if persisted.schema_version != BASKET_STORE_SCHEMA_VERSION {
            return Ok(BoltV3BasketRecoveryState::FailClosed {
                reason: BoltV3BasketRecoveryReason::UnsupportedSchemaVersion,
                state: Some(persisted.state),
            });
        }

        if persisted.state.status() == BoltV3BasketExecutionStatus::Stuck
            && persisted.state.unresolved_real_exposure()
        {
            return Ok(BoltV3BasketRecoveryState::FailClosed {
                reason: BoltV3BasketRecoveryReason::UnresolvedStuck,
                state: Some(persisted.state),
            });
        }

        Ok(BoltV3BasketRecoveryState::Recovered(persisted.state))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedBasketState {
    schema_version: u32,
    state: BoltV3BasketExecutionState,
}

#[derive(Debug)]
pub enum BoltV3BasketStoreError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Serialize(serde_json::Error),
    StateFileTooLarge {
        max_state_file_bytes: u64,
        actual_state_file_bytes: u64,
    },
}

impl From<AtomicIoError> for BoltV3BasketStoreError {
    fn from(error: AtomicIoError) -> Self {
        Self::Io {
            path: error.path,
            source: error.source,
        }
    }
}
