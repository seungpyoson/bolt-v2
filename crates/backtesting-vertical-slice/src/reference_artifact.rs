use std::{
    error::Error,
    fmt, fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{atomic_artifact_write::atomic_write, hashing::sha256_hex};

pub type Result<T> = std::result::Result<T, ReferenceArtifactError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceArtifactPin {
    pub role: String,
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceArtifactWrite {
    pub pin: ReferenceArtifactPin,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferenceArtifactError {
    Serialize(String),
    ReadExisting { path: String, error: String },
    ExistingArtifactMismatch { path: String },
    Write { path: String, error: String },
}

impl fmt::Display for ReferenceArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serialize(error) => write!(f, "serialize reference artifact: {error}"),
            Self::ReadExisting { path, error } => {
                write!(f, "read existing reference artifact {path}: {error}")
            }
            Self::ExistingArtifactMismatch { path } => {
                write!(
                    f,
                    "dirty reference artifact {path}: existing file content differs"
                )
            }
            Self::Write { path, error } => write!(f, "write reference artifact {path}: {error}"),
        }
    }
}

impl Error for ReferenceArtifactError {}

pub fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec_pretty(value)
        .map_err(|error| ReferenceArtifactError::Serialize(error.to_string()))
}

pub fn canonical_json_sha256<T: Serialize>(value: &T) -> Result<String> {
    let bytes = canonical_json_bytes(value)?;
    Ok(sha256_hex(&bytes))
}

pub fn write_reference_artifact<T: Serialize>(
    path: impl AsRef<Path>,
    role: impl Into<String>,
    value: &T,
) -> Result<ReferenceArtifactPin> {
    Ok(write_reference_artifact_with_len(path, role, value)?.pin)
}

pub fn write_reference_artifact_with_len<T: Serialize>(
    path: impl AsRef<Path>,
    role: impl Into<String>,
    value: &T,
) -> Result<ReferenceArtifactWrite> {
    write_reference_artifact_with_len_overwrite(path, role, value, false)
}

pub fn write_reference_artifact_with_len_overwrite<T: Serialize>(
    path: impl AsRef<Path>,
    role: impl Into<String>,
    value: &T,
    overwrite_existing: bool,
) -> Result<ReferenceArtifactWrite> {
    let path = path.as_ref();
    let bytes = canonical_json_bytes(value)?;
    if path.exists() {
        let existing = fs::read(path).map_err(|error| ReferenceArtifactError::ReadExisting {
            path: path.display().to_string(),
            error: error.to_string(),
        })?;
        if existing != bytes {
            if overwrite_existing {
                atomic_write(path, &bytes).map_err(|error| ReferenceArtifactError::Write {
                    path: path.display().to_string(),
                    error: error.to_string(),
                })?;
            } else {
                return Err(ReferenceArtifactError::ExistingArtifactMismatch {
                    path: path.display().to_string(),
                });
            }
        }
    } else {
        atomic_write(path, &bytes).map_err(|error| ReferenceArtifactError::Write {
            path: path.display().to_string(),
            error: error.to_string(),
        })?;
    }
    let sha256 = sha256_hex(&bytes);
    Ok(ReferenceArtifactWrite {
        pin: ReferenceArtifactPin {
            role: role.into(),
            path: path.to_path_buf(),
            sha256,
        },
        bytes: bytes.len() as u64,
    })
}

pub fn write_reference_artifact_with_len_mapped<
    T,
    E,
    SerializeError,
    ReadExistingError,
    MismatchError,
    WriteError,
>(
    path: impl AsRef<Path>,
    role: impl Into<String>,
    value: &T,
    serialize_error: SerializeError,
    read_existing_error: ReadExistingError,
    mismatch_error: MismatchError,
    write_error: WriteError,
) -> std::result::Result<ReferenceArtifactWrite, E>
where
    T: Serialize,
    SerializeError: FnOnce(String) -> E,
    ReadExistingError: FnOnce(String, String) -> E,
    MismatchError: FnOnce(String) -> E,
    WriteError: FnOnce(String, String) -> E,
{
    write_reference_artifact_with_len(path, role, value).map_err(|error| match error {
        ReferenceArtifactError::Serialize(error) => serialize_error(error),
        ReferenceArtifactError::ReadExisting { path, error } => read_existing_error(path, error),
        ReferenceArtifactError::ExistingArtifactMismatch { path } => mismatch_error(path),
        ReferenceArtifactError::Write { path, error } => write_error(path, error),
    })
}

pub fn write_reference_artifact_with_len_mapped_overwrite<
    T,
    E,
    SerializeError,
    ReadExistingError,
    MismatchError,
    WriteError,
>(
    path: impl AsRef<Path>,
    role: impl Into<String>,
    value: &T,
    overwrite_existing: bool,
    serialize_error: SerializeError,
    read_existing_error: ReadExistingError,
    mismatch_error: MismatchError,
    write_error: WriteError,
) -> std::result::Result<ReferenceArtifactWrite, E>
where
    T: Serialize,
    SerializeError: FnOnce(String) -> E,
    ReadExistingError: FnOnce(String, String) -> E,
    MismatchError: FnOnce(String) -> E,
    WriteError: FnOnce(String, String) -> E,
{
    write_reference_artifact_with_len_overwrite(path, role, value, overwrite_existing).map_err(
        |error| match error {
            ReferenceArtifactError::Serialize(error) => serialize_error(error),
            ReferenceArtifactError::ReadExisting { path, error } => {
                read_existing_error(path, error)
            }
            ReferenceArtifactError::ExistingArtifactMismatch { path } => mismatch_error(path),
            ReferenceArtifactError::Write { path, error } => write_error(path, error),
        },
    )
}
