use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use crate::bolt_v3_operator_artifacts::PRIVATE_ARTIFACT_FILE_MODE;

pub const PRIVATE_ATOMIC_FILE_MODE: u32 = PRIVATE_ARTIFACT_FILE_MODE;

#[derive(Debug)]
pub struct AtomicIoError {
    pub path: PathBuf,
    pub source: io::Error,
}

pub fn write_private_atomic_file(path: &Path, bytes: &[u8]) -> Result<(), AtomicIoError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| AtomicIoError {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let temp_path = private_atomic_temp_path(path);
    let result = write_private_synced_temp_file(&temp_path, bytes).and_then(|()| {
        fs::rename(&temp_path, path)
            .map_err(|source| AtomicIoError {
                path: path.to_path_buf(),
                source,
            })
            .and_then(|()| sync_parent_dir(path))
    });

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }

    result
}

pub fn private_atomic_temp_path(path: &Path) -> PathBuf {
    let mut temp_path = path.to_path_buf();
    temp_path.set_extension("tmp");
    temp_path
}

fn write_private_synced_temp_file(path: &Path, bytes: &[u8]) -> Result<(), AtomicIoError> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    configure_private_file_options(&mut options);

    let mut file = options.open(path).map_err(|source| AtomicIoError {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|source| AtomicIoError {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(unix)]
fn configure_private_file_options(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(PRIVATE_ATOMIC_FILE_MODE);
}

#[cfg(not(unix))]
fn configure_private_file_options(_options: &mut OpenOptions) {}

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
