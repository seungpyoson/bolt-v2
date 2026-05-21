use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub(crate) const CONFIG_FILE_SIZE_LIMIT_BYTES: u64 = 1_048_576;
const OVERSIZE_DETECTION_EXTRA_BYTE: u64 = 1;

#[derive(Debug)]
pub(crate) enum ConfigFileReadError {
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    TooLarge {
        path: PathBuf,
        length: u64,
        limit: u64,
    },
    NotRegularFile {
        path: PathBuf,
    },
    Utf8 {
        path: PathBuf,
        source: std::string::FromUtf8Error,
    },
}

impl std::fmt::Display for ConfigFileReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Open { path, source } => write!(f, "failed to open {}: {source}", path.display()),
            Self::Read { path, source } => write!(f, "failed to read {}: {source}", path.display()),
            Self::TooLarge {
                path,
                length,
                limit,
            } => write!(
                f,
                "{} exceeds config file size limit {limit} bytes (read at least {length} bytes)",
                path.display()
            ),
            Self::NotRegularFile { path } => {
                write!(f, "{} is not a regular config file", path.display())
            }
            Self::Utf8 { path, source } => {
                write!(f, "{} is not valid UTF-8: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigFileReadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Open { source, .. } | Self::Read { source, .. } => Some(source),
            Self::Utf8 { source, .. } => Some(source),
            Self::TooLarge { .. } | Self::NotRegularFile { .. } => None,
        }
    }
}

pub(crate) fn read_to_string(path: &Path) -> Result<String, ConfigFileReadError> {
    let file = File::open(path).map_err(|source| ConfigFileReadError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let metadata = file
        .metadata()
        .map_err(|source| ConfigFileReadError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.is_file() {
        return Err(ConfigFileReadError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    let mut bytes = Vec::new();
    file.take(CONFIG_FILE_SIZE_LIMIT_BYTES.saturating_add(OVERSIZE_DETECTION_EXTRA_BYTE))
        .read_to_end(&mut bytes)
        .map_err(|source| ConfigFileReadError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    bytes_to_string(path, bytes)
}

pub(crate) async fn read_to_string_async(path: &Path) -> Result<String, ConfigFileReadError> {
    use tokio::io::AsyncReadExt;

    let file = tokio::fs::File::open(path)
        .await
        .map_err(|source| ConfigFileReadError::Open {
            path: path.to_path_buf(),
            source,
        })?;
    let metadata = file
        .metadata()
        .await
        .map_err(|source| ConfigFileReadError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if !metadata.is_file() {
        return Err(ConfigFileReadError::NotRegularFile {
            path: path.to_path_buf(),
        });
    }
    let mut bytes = Vec::new();
    file.take(CONFIG_FILE_SIZE_LIMIT_BYTES.saturating_add(OVERSIZE_DETECTION_EXTRA_BYTE))
        .read_to_end(&mut bytes)
        .await
        .map_err(|source| ConfigFileReadError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    bytes_to_string(path, bytes)
}

fn bytes_to_string(path: &Path, bytes: Vec<u8>) -> Result<String, ConfigFileReadError> {
    let length = bytes.len() as u64;
    if length > CONFIG_FILE_SIZE_LIMIT_BYTES {
        return Err(ConfigFileReadError::TooLarge {
            path: path.to_path_buf(),
            length,
            limit: CONFIG_FILE_SIZE_LIMIT_BYTES,
        });
    }
    String::from_utf8(bytes).map_err(|source| ConfigFileReadError::Utf8 {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CONFIG_FILE_SIZE_LIMIT_BYTES, ConfigFileReadError, read_to_string, read_to_string_async,
    };

    #[test]
    fn sync_reader_accepts_config_exactly_at_size_limit() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = temp.path().join("exact-limit-root.toml");
        std::fs::write(&path, vec![b'x'; CONFIG_FILE_SIZE_LIMIT_BYTES as usize])
            .expect("exact-limit config fixture should write");

        let contents =
            read_to_string(&path).expect("sync config reader must accept exact-limit files");

        assert_eq!(contents.len() as u64, CONFIG_FILE_SIZE_LIMIT_BYTES);
    }

    #[test]
    fn sync_reader_rejects_directory_path() {
        let temp = tempfile::tempdir().expect("tempdir should create");

        let error = read_to_string(temp.path())
            .expect_err("sync config reader must reject non-regular files before reading");

        assert!(
            matches!(error, ConfigFileReadError::NotRegularFile { .. }),
            "expected NotRegularFile, got {error:?}"
        );
    }

    #[test]
    fn sync_reader_rejects_config_over_size_limit() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = temp.path().join("oversized-root.toml");
        std::fs::write(&path, vec![b'x'; CONFIG_FILE_SIZE_LIMIT_BYTES as usize + 1])
            .expect("oversized config fixture should write");

        let error = read_to_string(&path)
            .expect_err("sync config reader must reject oversized config files");

        match error {
            ConfigFileReadError::TooLarge { length, limit, .. } => {
                assert_eq!(limit, CONFIG_FILE_SIZE_LIMIT_BYTES);
                assert_eq!(length, CONFIG_FILE_SIZE_LIMIT_BYTES + 1);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[test]
    fn sync_reader_rejects_invalid_utf8() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = temp.path().join("invalid-root.toml");
        std::fs::write(&path, [0xff, 0xfe]).expect("invalid UTF-8 fixture should write");

        let error =
            read_to_string(&path).expect_err("sync config reader must reject invalid UTF-8");

        assert!(
            matches!(error, ConfigFileReadError::Utf8 { .. }),
            "expected Utf8, got {error:?}"
        );
    }

    #[tokio::test]
    async fn async_reader_accepts_config_exactly_at_size_limit() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = temp.path().join("exact-limit-root.toml");
        std::fs::write(&path, vec![b'x'; CONFIG_FILE_SIZE_LIMIT_BYTES as usize])
            .expect("exact-limit config fixture should write");

        let contents = read_to_string_async(&path)
            .await
            .expect("async config reader must accept exact-limit files");

        assert_eq!(contents.len() as u64, CONFIG_FILE_SIZE_LIMIT_BYTES);
    }

    #[tokio::test]
    async fn async_reader_rejects_directory_path() {
        let temp = tempfile::tempdir().expect("tempdir should create");

        let error = read_to_string_async(temp.path())
            .await
            .expect_err("async config reader must reject non-regular files before reading");

        assert!(
            matches!(error, ConfigFileReadError::NotRegularFile { .. }),
            "expected NotRegularFile, got {error:?}"
        );
    }

    #[tokio::test]
    async fn async_reader_rejects_config_over_size_limit() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = temp.path().join("oversized-root.toml");
        std::fs::write(&path, vec![b'x'; CONFIG_FILE_SIZE_LIMIT_BYTES as usize + 1])
            .expect("oversized config fixture should write");

        let error = read_to_string_async(&path)
            .await
            .expect_err("async config reader must reject oversized config files");

        match error {
            ConfigFileReadError::TooLarge { length, limit, .. } => {
                assert_eq!(limit, CONFIG_FILE_SIZE_LIMIT_BYTES);
                assert_eq!(length, CONFIG_FILE_SIZE_LIMIT_BYTES + 1);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn async_reader_rejects_invalid_utf8() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let path = temp.path().join("invalid-root.toml");
        std::fs::write(&path, [0xff, 0xfe]).expect("invalid UTF-8 fixture should write");

        let error = read_to_string_async(&path)
            .await
            .expect_err("async config reader must reject invalid UTF-8");

        assert!(
            matches!(error, ConfigFileReadError::Utf8 { .. }),
            "expected Utf8, got {error:?}"
        );
    }
}
