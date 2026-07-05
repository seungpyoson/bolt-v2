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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceArtifactRewrite {
    FailOnDirty,
    Overwrite,
}

pub struct ReferenceArtifactErrorMappers<
    SerializeError,
    ReadExistingError,
    MismatchError,
    WriteError,
> {
    pub serialize_error: SerializeError,
    pub read_existing_error: ReadExistingError,
    pub mismatch_error: MismatchError,
    pub write_error: WriteError,
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
    rewrite: ReferenceArtifactRewrite,
) -> Result<ReferenceArtifactPin> {
    Ok(write_reference_artifact_with_len(path, role, value, rewrite)?.pin)
}

pub fn write_reference_artifact_with_len<T: Serialize>(
    path: impl AsRef<Path>,
    role: impl Into<String>,
    value: &T,
    rewrite: ReferenceArtifactRewrite,
) -> Result<ReferenceArtifactWrite> {
    let path = path.as_ref();
    let bytes = canonical_json_bytes(value)?;
    if path.exists() {
        let existing = fs::read(path).map_err(|error| ReferenceArtifactError::ReadExisting {
            path: path.display().to_string(),
            error: error.to_string(),
        })?;
        if existing != bytes {
            match rewrite {
                ReferenceArtifactRewrite::FailOnDirty => {
                    return Err(ReferenceArtifactError::ExistingArtifactMismatch {
                        path: path.display().to_string(),
                    });
                }
                ReferenceArtifactRewrite::Overwrite => {
                    atomic_write(path, &bytes).map_err(|error| ReferenceArtifactError::Write {
                        path: path.display().to_string(),
                        error: error.to_string(),
                    })?;
                }
            }
        } else if rewrite == ReferenceArtifactRewrite::Overwrite {
            atomic_write(path, &bytes).map_err(|error| ReferenceArtifactError::Write {
                path: path.display().to_string(),
                error: error.to_string(),
            })?;
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
    rewrite: ReferenceArtifactRewrite,
    mappers: ReferenceArtifactErrorMappers<
        SerializeError,
        ReadExistingError,
        MismatchError,
        WriteError,
    >,
) -> std::result::Result<ReferenceArtifactWrite, E>
where
    T: Serialize,
    SerializeError: FnOnce(String) -> E,
    ReadExistingError: FnOnce(String, String) -> E,
    MismatchError: FnOnce(String) -> E,
    WriteError: FnOnce(String, String) -> E,
{
    write_reference_artifact_with_len(path, role, value, rewrite)
        .map_err(|error| map_reference_artifact_error(error, mappers))
}

fn map_reference_artifact_error<E, SerializeError, ReadExistingError, MismatchError, WriteError>(
    error: ReferenceArtifactError,
    mappers: ReferenceArtifactErrorMappers<
        SerializeError,
        ReadExistingError,
        MismatchError,
        WriteError,
    >,
) -> E
where
    SerializeError: FnOnce(String) -> E,
    ReadExistingError: FnOnce(String, String) -> E,
    MismatchError: FnOnce(String) -> E,
    WriteError: FnOnce(String, String) -> E,
{
    let ReferenceArtifactErrorMappers {
        serialize_error,
        read_existing_error,
        mismatch_error,
        write_error,
    } = mappers;
    match error {
        ReferenceArtifactError::Serialize(error) => serialize_error(error),
        ReferenceArtifactError::ReadExisting { path, error } => read_existing_error(path, error),
        ReferenceArtifactError::ExistingArtifactMismatch { path } => mismatch_error(path),
        ReferenceArtifactError::Write { path, error } => write_error(path, error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fail_on_dirty_rejects_mismatched_existing_artifact() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("artifact.json");
        write_reference_artifact_with_len(
            &path,
            "test-role",
            &json!({"value": "first"}),
            ReferenceArtifactRewrite::FailOnDirty,
        )
        .unwrap();
        fs::write(&path, br#"{"dirty":true}"#).unwrap();

        let err = write_reference_artifact_with_len(
            &path,
            "test-role",
            &json!({"value": "second"}),
            ReferenceArtifactRewrite::FailOnDirty,
        )
        .unwrap_err();

        assert_eq!(
            err,
            ReferenceArtifactError::ExistingArtifactMismatch {
                path: path.display().to_string()
            }
        );
    }

    #[test]
    fn overwrite_refreshes_bytes_and_returns_new_pin_hash() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("artifact.json");
        write_reference_artifact_with_len(
            &path,
            "test-role",
            &json!({"value": "first"}),
            ReferenceArtifactRewrite::Overwrite,
        )
        .unwrap();
        fs::write(&path, br#"{"dirty":true}"#).unwrap();
        let replacement = json!({"value": "second"});
        let expected_bytes = canonical_json_bytes(&replacement).unwrap();

        let written = write_reference_artifact_with_len(
            &path,
            "test-role",
            &replacement,
            ReferenceArtifactRewrite::Overwrite,
        )
        .unwrap();

        assert_eq!(fs::read(&path).unwrap(), expected_bytes);
        assert_eq!(written.pin.sha256, sha256_hex(&expected_bytes));
        assert_eq!(written.bytes, expected_bytes.len() as u64);
    }
}
