use std::{
    error::Error,
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub(crate) const PRIVATE_ARTIFACT_FILE_MODE: u32 = 0o600;
pub const ENTRY_DECISION_ZERO_TIMESTAMP_MS: u64 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrittenOperatorArtifact {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug)]
pub enum BoltV3OperatorArtifactError {
    UnsupportedProvider {
        client_key: String,
        provider_key: String,
    },
    ProviderArtifactInvalid {
        artifact: &'static str,
        field: &'static str,
    },
    PreRunClobV2SourceInvalid {
        field: &'static str,
    },
    PreRunVenueAccountStateSourceInvalid {
        field: &'static str,
    },
    DecisionEvidenceSourceInvalid {
        message: String,
    },
    Serialize(serde_json::Error),
    Write {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for BoltV3OperatorArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedProvider {
                client_key,
                provider_key,
            } => write!(
                f,
                "client `{client_key}` uses unsupported provider `{provider_key}`"
            ),
            Self::ProviderArtifactInvalid { artifact, field } => {
                write!(f, "{artifact} artifact field `{field}` is invalid")
            }
            Self::PreRunClobV2SourceInvalid { field } => {
                write!(f, "pre-run CLOB v2 source field `{field}` is invalid")
            }
            Self::PreRunVenueAccountStateSourceInvalid { field } => {
                write!(
                    f,
                    "pre-run venue account state source field `{field}` is invalid"
                )
            }
            Self::DecisionEvidenceSourceInvalid { message } => f.write_str(message),
            Self::Serialize(source) => write!(f, "failed to serialize artifact: {source}"),
            Self::Write { path, source } => {
                write!(f, "failed to write artifact `{}`: {source}", path.display())
            }
        }
    }
}

impl Error for BoltV3OperatorArtifactError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Serialize(source) => Some(source),
            Self::Write { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn is_lowercase_git_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowercase_hash_shape_helpers_reject_wrong_width_and_uppercase() {
        assert!(is_lowercase_git_sha(
            "0123456789abcdef0123456789abcdef01234567"
        ));
        assert!(!is_lowercase_git_sha(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_lowercase_git_sha(
            "0123456789ABCDEF0123456789abcdef01234567"
        ));

        assert!(is_lowercase_sha256(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_lowercase_sha256(
            "0123456789abcdef0123456789abcdef01234567"
        ));
        assert!(!is_lowercase_sha256(
            "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
    }
}

pub fn json_artifact_sha256<T: Serialize>(
    artifact: &T,
) -> Result<String, BoltV3OperatorArtifactError> {
    let bytes =
        serde_json::to_vec_pretty(artifact).map_err(BoltV3OperatorArtifactError::Serialize)?;
    Ok(sha256_hex(&bytes))
}

pub fn write_json_artifact_create_new<T: Serialize>(
    path: &Path,
    artifact: &T,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let mut bytes =
        serde_json::to_vec_pretty(artifact).map_err(BoltV3OperatorArtifactError::Serialize)?;
    bytes.push(b'\n');
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| BoltV3OperatorArtifactError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(PRIVATE_ARTIFACT_FILE_MODE);
    }
    let mut file = options
        .open(path)
        .map_err(|source| BoltV3OperatorArtifactError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|source| BoltV3OperatorArtifactError::Write {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(WrittenOperatorArtifact {
        path: path.to_path_buf(),
        sha256: sha256_hex(&bytes),
    })
}

pub fn read_file_bounded(path: &Path, max_bytes: u64) -> io::Result<Vec<u8>> {
    let mut file = fs::File::open(path)?;
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "artifact exceeds configured byte limit",
        ));
    }
    Ok(bytes)
}

pub fn entry_decision_source_invalid(message: impl Into<String>) -> BoltV3OperatorArtifactError {
    BoltV3OperatorArtifactError::DecisionEvidenceSourceInvalid {
        message: message.into(),
    }
}

pub fn price_to_beat_report_provenance_invalid() -> BoltV3OperatorArtifactError {
    entry_decision_source_invalid("price-to-beat report provenance is invalid")
}

pub fn price_to_beat_report_provenance_config_invalid() -> BoltV3OperatorArtifactError {
    entry_decision_source_invalid("price-to-beat report provenance config is invalid")
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
