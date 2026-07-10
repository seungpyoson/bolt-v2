use std::{
    error::Error,
    fmt, fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bolt_v3_deploy_target::ObservedHostFacts;

pub(crate) const PRIVATE_ARTIFACT_FILE_MODE: u32 = 0o600;
pub const ENTRY_DECISION_ZERO_TIMESTAMP_MS: u64 = 0;
const LAUNCH_IDENTITY_FILE_NAME: &str = "launch-identity.json";
const LAUNCH_IDENTITY_MAX_BYTES: u64 = 64 * 1024;

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
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_json::Error,
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
            Self::Read { path, source } => {
                write!(f, "failed to read artifact `{}`: {source}", path.display())
            }
            Self::Parse { path, source } => {
                write!(f, "failed to parse artifact `{}`: {source}", path.display())
            }
        }
    }
}

impl Error for BoltV3OperatorArtifactError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Serialize(source) => Some(source),
            Self::Write { source, .. } => Some(source),
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LaunchIdentity {
    /// Git HEAD SHA the running binary was built from, or `None` when the
    /// build did not embed it (see `current_build_head_sha`).
    pub build_head_sha: Option<String>,
    /// Profile name this launch was started with.
    pub profile: String,
    /// Checksum of the exact config bundle that was loaded for this launch.
    pub config_bundle_checksum: String,
    /// Wall-clock launch time, seconds since the Unix epoch.
    pub launched_at_unix_secs: u64,
    /// OS process id of the launching process.
    pub pid: u32,
    /// Host facts observed at launch, `None` when no deploy target was
    /// configured (no host was observed).
    pub target_host_facts: Option<ObservedHostFacts>,
}

pub fn launch_identity_path(catalog_directory: &Path) -> PathBuf {
    catalog_directory.join(LAUNCH_IDENTITY_FILE_NAME)
}

pub fn write_launch_identity(
    catalog_directory: &Path,
    identity: &LaunchIdentity,
) -> Result<WrittenOperatorArtifact, BoltV3OperatorArtifactError> {
    let path = launch_identity_path(catalog_directory);
    let mut bytes =
        serde_json::to_vec_pretty(identity).map_err(BoltV3OperatorArtifactError::Serialize)?;
    bytes.push(b'\n');
    crate::bolt_v3_atomic_io::write_atomic_file_with_mode(
        &path,
        &bytes,
        crate::bolt_v3_atomic_io::GROUP_READABLE_ARTIFACT_FILE_MODE,
    )
    .map_err(|error| BoltV3OperatorArtifactError::Write {
        path: error.path,
        source: error.source,
    })?;
    Ok(WrittenOperatorArtifact {
        path,
        sha256: sha256_hex(&bytes),
    })
}

pub fn read_launch_identity(
    catalog_directory: &Path,
) -> Result<Option<LaunchIdentity>, BoltV3OperatorArtifactError> {
    let path = launch_identity_path(catalog_directory);
    let bytes = match read_file_bounded(&path, LAUNCH_IDENTITY_MAX_BYTES) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(BoltV3OperatorArtifactError::Read { path, source }),
    };
    let identity = serde_json::from_slice(&bytes)
        .map_err(|source| BoltV3OperatorArtifactError::Parse { path, source })?;
    Ok(Some(identity))
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

pub fn current_build_head_sha() -> Option<&'static str> {
    option_env!("BOLT_V3_BUILD_HEAD_SHA").filter(|value| is_lowercase_git_sha(value))
}

pub fn build_head_sha_matches_current(value: &str) -> bool {
    current_build_head_sha().is_some_and(|build_head_sha| value == build_head_sha)
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

    #[test]
    fn current_build_head_sha_is_valid_when_emitted() {
        if let Some(build_head_sha) = current_build_head_sha() {
            assert!(is_lowercase_git_sha(build_head_sha));
            assert!(build_head_sha_matches_current(build_head_sha));
        }
    }

    #[test]
    fn launch_identity_round_trips_through_catalog_directory() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let identity = LaunchIdentity {
            build_head_sha: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            profile: "round-trip-profile".to_string(),
            config_bundle_checksum:
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            launched_at_unix_secs: 1_700_000_000,
            pid: 4242,
            target_host_facts: None,
        };
        write_launch_identity(temp.path(), &identity).expect("write should succeed");
        let read_back = read_launch_identity(temp.path()).expect("read should succeed");
        assert_eq!(read_back, Some(identity));
    }

    #[test]
    fn launch_identity_round_trips_with_observed_host_facts() {
        // Every other round-trip uses `target_host_facts: None`; this exercises
        // the `Some(ObservedHostFacts { .. })` serde path of the durable artifact,
        // including a `None` nested field, so the nested option round-trips too.
        let temp = tempfile::tempdir().expect("tempdir should create");
        let identity = LaunchIdentity {
            build_head_sha: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            profile: "facts-round-trip-profile".to_string(),
            config_bundle_checksum:
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            launched_at_unix_secs: 1_700_000_001,
            pid: 4243,
            target_host_facts: Some(ObservedHostFacts {
                region: Some("test-region".to_string()),
                availability_zone: None,
                instance_id: Some("test-instance".to_string()),
            }),
        };
        write_launch_identity(temp.path(), &identity).expect("write should succeed");
        let read_back = read_launch_identity(temp.path()).expect("read should succeed");
        assert_eq!(read_back, Some(identity));
    }

    #[test]
    fn read_launch_identity_is_none_when_file_absent() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let read_back = read_launch_identity(temp.path()).expect("read should succeed");
        assert_eq!(read_back, None);
    }

    #[test]
    fn read_launch_identity_rejects_oversize_artifact() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        // One byte past the bounded reader's ceiling. The size check precedes
        // parsing, so the payload does not need to be valid JSON: an oversize
        // artifact must surface as a loud `Read` error, never `Ok(None)` (which
        // would mask the artifact) nor `Ok(Some)` (which would trust it).
        let oversize = vec![b'x'; LAUNCH_IDENTITY_MAX_BYTES as usize + 1];
        std::fs::write(launch_identity_path(temp.path()), &oversize)
            .expect("oversize fixture should write");
        match read_launch_identity(temp.path()) {
            Err(BoltV3OperatorArtifactError::Read { .. }) => {}
            other => panic!("expected Read error for oversize artifact, got {other:?}"),
        }
    }

    #[test]
    fn write_launch_identity_overwrites_previous() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let first = LaunchIdentity {
            build_head_sha: None,
            profile: "first-profile".to_string(),
            config_bundle_checksum: "aaaa".to_string(),
            launched_at_unix_secs: 1,
            pid: 4242,
            target_host_facts: None,
        };
        let second = LaunchIdentity {
            build_head_sha: Some("0123456789abcdef0123456789abcdef01234567".to_string()),
            profile: "second-profile".to_string(),
            config_bundle_checksum: "bbbb".to_string(),
            launched_at_unix_secs: 2,
            pid: 4242,
            target_host_facts: None,
        };
        write_launch_identity(temp.path(), &first).expect("first write should succeed");
        write_launch_identity(temp.path(), &second).expect("second write should succeed");
        let read_back = read_launch_identity(temp.path()).expect("read should succeed");
        assert_eq!(read_back, Some(second));
    }

    #[test]
    fn write_launch_identity_uses_group_readable_not_world_readable_mode() {
        // The artifact carries host-identifying metadata (pid + region/AZ/
        // instance-id), so it must be owner+group readable for `ops status` but
        // NOT world-readable. Pin the on-disk mode to 0640 so a regression back to
        // the world-readable runtime-config mode (0644) fails loudly.
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().expect("tempdir should create");
        let identity = LaunchIdentity {
            build_head_sha: None,
            profile: "mode-profile".to_string(),
            config_bundle_checksum: "cccc".to_string(),
            launched_at_unix_secs: 1,
            pid: 4242,
            target_host_facts: None,
        };
        write_launch_identity(temp.path(), &identity).expect("write should succeed");
        let mode = std::fs::metadata(launch_identity_path(temp.path()))
            .expect("artifact metadata should read")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o640,
            "launch-identity artifact must be group-readable but not world-readable"
        );
    }

    #[test]
    fn launch_identity_rejects_unknown_fields() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = launch_identity_path(temp.path());
        std::fs::write(
            &path,
            br#"{"build_head_sha":null,"profile":"p","config_bundle_checksum":"c","launched_at_unix_secs":1,"pid":1,"target_host_facts":null,"unexpected":true}"#,
        )
        .expect("write fixture should succeed");
        match read_launch_identity(temp.path()) {
            Err(BoltV3OperatorArtifactError::Parse { .. }) => {}
            other => panic!("expected Parse error for unknown field, got {other:?}"),
        }
    }

    #[test]
    fn launch_identity_v1_old_bytes_remain_readable() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        std::fs::write(
            launch_identity_path(temp.path()),
            include_bytes!("../tests/fixtures/bolt_v3/compatibility/launch_identity_v1.json"),
        )
        .expect("old-byte launch identity fixture should write");

        let identity = read_launch_identity(temp.path())
            .expect("old-byte launch identity should parse")
            .expect("old-byte launch identity should exist");
        assert_eq!(identity.profile, "legacy-profile");
        assert_eq!(identity.pid, 4242);
        assert_eq!(identity.launched_at_unix_secs, 1_700_000_000);
        assert_eq!(
            identity.build_head_sha.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert_eq!(
            identity.config_bundle_checksum,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert!(identity.target_host_facts.is_none());
    }

    #[test]
    fn launch_identity_path_is_under_catalog_directory_with_expected_name() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = launch_identity_path(temp.path());
        assert_eq!(path, temp.path().join("launch-identity.json"));
        assert!(path.ends_with("launch-identity.json"));
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
