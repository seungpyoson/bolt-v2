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
            Self::TooLarge { .. } => None,
        }
    }
}

pub(crate) fn read_to_string(path: &Path) -> Result<String, ConfigFileReadError> {
    let file = File::open(path).map_err(|source| ConfigFileReadError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(CONFIG_FILE_SIZE_LIMIT_BYTES.saturating_add(OVERSIZE_DETECTION_EXTRA_BYTE))
        .read_to_end(&mut bytes)
        .map_err(|source| ConfigFileReadError::Read {
            path: path.to_path_buf(),
            source,
        })?;
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
