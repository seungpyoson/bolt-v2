use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::bolt_v3_operator_artifacts::PRIVATE_ARTIFACT_FILE_MODE;

pub const PRIVATE_ATOMIC_FILE_MODE: u32 = PRIVATE_ARTIFACT_FILE_MODE;

/// Mode for non-secret files a service user must read (e.g. the generated runtime
/// config, which carries only SSM references and public addresses). Group- and
/// world-readable so the `bolt` service user can read it regardless of ownership;
/// the deploy may further tighten ownership/mode to root:bolt 0640.
pub const RUNTIME_CONFIG_FILE_MODE: u32 = 0o644;

#[derive(Debug)]
pub struct AtomicIoError {
    pub path: PathBuf,
    pub source: io::Error,
}

static PRIVATE_ATOMIC_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn write_private_atomic_file(path: &Path, bytes: &[u8]) -> Result<(), AtomicIoError> {
    write_atomic_file_with_mode(path, bytes, PRIVATE_ATOMIC_FILE_MODE)
}

/// Atomically write `bytes` to `path` with the given Unix `mode` (create new temp +
/// fsync + rename + parent fsync). Use [`PRIVATE_ATOMIC_FILE_MODE`] for secret-bearing
/// artifacts and [`RUNTIME_CONFIG_FILE_MODE`] for non-secret files a service user reads.
pub fn write_atomic_file_with_mode(
    path: &Path,
    bytes: &[u8],
    mode: u32,
) -> Result<(), AtomicIoError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| AtomicIoError {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let temp_path = private_atomic_temp_path_for_write(path);
    if let Err(error) = write_synced_temp_file(&temp_path, bytes, mode) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    if let Err(source) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(AtomicIoError {
            path: path.to_path_buf(),
            source,
        });
    }

    sync_parent_dir(path)?;
    Ok(())
}

pub fn private_atomic_temp_path(path: &Path) -> PathBuf {
    private_atomic_temp_path_with_suffix(path, "tmp")
}

fn private_atomic_temp_path_for_write(path: &Path) -> PathBuf {
    let counter = PRIVATE_ATOMIC_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let timestamp_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    private_atomic_temp_path_with_suffix(
        path,
        &format!("tmp.{}.{}.{}", std::process::id(), timestamp_ns, counter),
    )
}

fn private_atomic_temp_path_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut temp_path = path.as_os_str().to_os_string();
    temp_path.push(".");
    temp_path.push(suffix);
    PathBuf::from(temp_path)
}

fn write_synced_temp_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), AtomicIoError> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    configure_file_options(&mut options, mode);

    let mut file = options.open(path).map_err(|source| AtomicIoError {
        path: path.to_path_buf(),
        source,
    })?;
    // `OpenOptionsExt::mode` only requests the create mode; the kernel masks it with the
    // process umask, so a restrictive deploy umask (e.g. 077) would silently downgrade a
    // 0644 runtime config to 0600 and lock the `bolt` service user out — the exact #768
    // failure class. Set the final mode explicitly after create so it is umask-independent.
    enforce_exact_file_mode(&file, path, mode)?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| AtomicIoError {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(unix)]
fn configure_file_options(options: &mut OpenOptions, mode: u32) {
    use std::os::unix::fs::OpenOptionsExt;
    // Bound the create-time mode so the brief pre-chmod window is never *more* permissive
    // than requested; `enforce_exact_file_mode` then pins the exact mode regardless of umask.
    options.mode(mode);
}

#[cfg(not(unix))]
fn configure_file_options(_options: &mut OpenOptions, _mode: u32) {}

#[cfg(unix)]
fn enforce_exact_file_mode(file: &fs::File, path: &Path, mode: u32) -> Result<(), AtomicIoError> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(mode))
        .map_err(|source| AtomicIoError {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn enforce_exact_file_mode(
    _file: &fs::File,
    _path: &Path,
    _mode: u32,
) -> Result<(), AtomicIoError> {
    Ok(())
}

fn sync_parent_dir(path: &Path) -> Result<(), AtomicIoError> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|source| AtomicIoError {
            path: parent.to_path_buf(),
            source,
        })
}
