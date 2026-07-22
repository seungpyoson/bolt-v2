use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, anyhow, bail, ensure};

use crate::{
    bolt_v3_config::LoadedBoltV3Config, bolt_v3_operator_artifacts::PRIVATE_ARTIFACT_FILE_MODE,
};

use super::{
    facts::StartupRecoveryFacts, reader::validate_machine_stream, record::DecisionEvidenceRecorder,
    validate_relative_path,
};

#[derive(Debug)]
pub struct DecisionEvidenceRuntime {
    recorder: Arc<DecisionEvidenceRecorder>,
    startup_recovery: Arc<StartupRecoveryFacts>,
}

/// File-backed current-evidence runtime exposed only to the separately resolved backtesting
/// workspace. The live package does not enable this feature, so production append capability still
/// has exactly one constructor: [`DecisionEvidenceRuntime::open`].
#[cfg(feature = "offline-current-evidence")]
#[derive(Debug)]
pub struct OfflineDecisionEvidenceRuntime {
    recorder: Arc<DecisionEvidenceRecorder>,
}

#[cfg(feature = "offline-current-evidence")]
impl OfflineDecisionEvidenceRuntime {
    /// Opens two fresh isolated files through the production recorder and sink implementation.
    pub fn from_fresh_files(
        mut machine: File,
        observation: File,
        reject_episode_max_count: usize,
    ) -> Result<Self> {
        ensure!(
            reject_episode_max_count > 0,
            "reject_episode_max_count must be positive"
        );
        ensure!(
            machine.metadata()?.len() == 0 && observation.metadata()?.len() == 0,
            "offline current-evidence streams must be fresh"
        );
        ensure_distinct_files(&machine, &observation)?;
        let recovery = validate_machine_stream(&mut machine, Some(0))?;
        ensure!(
            recovery.is_empty(),
            "fresh offline current-evidence stream produced recovery state"
        );
        machine.seek(SeekFrom::End(0))?;
        Ok(Self {
            recorder: Arc::new(DecisionEvidenceRecorder::from_files(
                machine,
                observation,
                reject_episode_max_count,
            )),
        })
    }

    /// Returns the same concrete recorder type used by live strategy construction.
    #[must_use]
    pub fn recorder(&self) -> Arc<DecisionEvidenceRecorder> {
        Arc::clone(&self.recorder)
    }
}

impl DecisionEvidenceRuntime {
    pub fn open(loaded: &LoadedBoltV3Config) -> Result<Self> {
        let config = &loaded.root.persistence.decision_evidence;
        let catalog = fs::canonicalize(&loaded.root.persistence.catalog_directory)
            .context("canonicalize decision-evidence catalog_directory")?;
        ensure!(
            catalog.is_dir(),
            "decision-evidence catalog_directory is not a directory"
        );

        let machine_path = resolve_active_path(
            &catalog,
            "machine_relative_path",
            &config.machine_relative_path,
        )?;
        let observation_path = resolve_active_path(
            &catalog,
            "observation_relative_path",
            &config.observation_relative_path,
        )?;
        ensure!(
            machine_path != observation_path,
            "decision-evidence paths must be distinct"
        );

        let mut configured = BTreeSet::from([machine_path.clone(), observation_path.clone()]);
        for retired in &config.retired_relative_paths {
            let retired_path =
                resolve_configured_path(&catalog, "retired_relative_paths", retired)?;
            ensure!(
                configured.insert(retired_path.clone()),
                "decision-evidence paths must be distinct: `{}`",
                retired_path.display()
            );
            match fs::symlink_metadata(&retired_path) {
                Ok(_) => bail!(
                    "retired decision-evidence path is present: `{}`",
                    retired_path.display()
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "inspect retired decision-evidence path `{}`",
                            retired_path.display()
                        )
                    });
                }
            }
        }

        let mut machine = open_retained_stream(&machine_path)
            .with_context(|| format!("open machine evidence `{}`", machine_path.display()))?;
        let startup_recovery =
            validate_machine_stream(&mut machine, Some(config.recovery_evidence_max_bytes))?;
        machine.seek(SeekFrom::End(0))?;
        let observation = open_retained_stream(&observation_path).with_context(|| {
            format!("open observation evidence `{}`", observation_path.display())
        })?;
        ensure_distinct_files(&machine, &observation)?;

        Ok(Self {
            recorder: Arc::new(DecisionEvidenceRecorder::from_files(
                machine,
                observation,
                config.reject_episode_max_count,
            )),
            startup_recovery: Arc::new(startup_recovery),
        })
    }

    #[must_use]
    pub fn recorder(&self) -> Arc<DecisionEvidenceRecorder> {
        Arc::clone(&self.recorder)
    }

    #[must_use]
    pub fn startup_recovery(&self) -> Arc<StartupRecoveryFacts> {
        Arc::clone(&self.startup_recovery)
    }
}

fn resolve_active_path(catalog: &Path, field: &str, raw: &str) -> Result<PathBuf> {
    let path = resolve_configured_path(catalog, field, raw)?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("decision-evidence path has no parent"))?;
    let canonical_parent = fs::canonicalize(parent).with_context(|| {
        format!(
            "canonicalize decision-evidence parent `{}`; cutover must create it before startup",
            parent.display()
        )
    })?;
    ensure!(
        canonical_parent.starts_with(catalog),
        "decision-evidence path escapes catalog_directory"
    );
    Ok(path)
}

fn resolve_configured_path(catalog: &Path, field: &str, raw: &str) -> Result<PathBuf> {
    validate_relative_path(field, raw).map_err(|message| anyhow!(message))?;
    Ok(catalog.join(raw.trim()))
}

fn open_retained_stream(path: &Path) -> std::io::Result<File> {
    match fs::symlink_metadata(path) {
        Ok(before) => {
            validate_regular_file(&before)?;
            let file = open_existing_no_follow(path)?;
            let opened = file.metadata()?;
            validate_regular_file(&opened)?;
            ensure_same_file(&before, &opened)?;
            let after = fs::symlink_metadata(path)?;
            validate_regular_file(&after)?;
            ensure_same_file(&opened, &after)?;
            Ok(file)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let file = open_new_no_follow(path)?;
            let opened = file.metadata()?;
            validate_regular_file(&opened)?;
            let after = fs::symlink_metadata(path)?;
            validate_regular_file(&after)?;
            ensure_same_file(&opened, &after)?;
            Ok(file)
        }
        Err(error) => Err(error),
    }
}

fn validate_regular_file(metadata: &fs::Metadata) -> std::io::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "decision-evidence path is not a regular file",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_existing_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .append(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_existing_no_follow(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).append(true).open(path)
}

#[cfg(unix)]
fn open_new_no_follow(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .append(true)
        .create_new(true)
        .mode(PRIVATE_ARTIFACT_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(not(unix))]
fn open_new_no_follow(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .append(true)
        .create_new(true)
        .open(path)
}

#[cfg(unix)]
fn ensure_same_file(left: &fs::Metadata, right: &fs::Metadata) -> std::io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    if left.dev() != right.dev() || left.ino() != right.ino() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "decision-evidence path changed during open",
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_same_file(_left: &fs::Metadata, _right: &fs::Metadata) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn ensure_distinct_files(machine: &File, observation: &File) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let machine = machine.metadata()?;
    let observation = observation.metadata()?;
    ensure!(
        machine.dev() != observation.dev() || machine.ino() != observation.ino(),
        "machine and observation evidence paths resolve to the same file"
    );
    Ok(())
}

#[cfg(not(unix))]
fn ensure_distinct_files(_machine: &File, _observation: &File) -> Result<()> {
    Ok(())
}
