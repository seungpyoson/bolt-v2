use std::{
    fs::File,
    io::{Seek, SeekFrom},
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result, ensure};

use crate::bolt_v3_config::LoadedBoltV3Config;

use super::{
    CanonicalRelativeEvidencePath, PositiveFiniteEvidenceReadCap,
    facts::{BookingRecoveryFacts, ReservationRecoveryFacts, SettlementRecoveryFacts},
    generated_contract::KnownSink,
    path_authority::CatalogDirectory,
    reader::validate_stream,
    record::{DecisionEvidenceRecorder, ObservationStreamStatus, PoisonCause},
};

#[derive(Debug)]
pub struct DecisionEvidenceRuntime {
    recorder: Arc<DecisionEvidenceRecorder>,
    reservation_recovery: Arc<ReservationRecoveryFacts>,
    settlement_recovery: Arc<SettlementRecoveryFacts>,
    booking_recovery: Arc<BookingRecoveryFacts>,
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
    /// Opens two fresh isolated streams through the production catalog authority and recorder.
    pub fn open_isolated(
        catalog_directory: &Path,
        machine_relative_path: &str,
        observation_relative_path: &str,
        recovery_evidence_max_bytes: PositiveFiniteEvidenceReadCap,
        reject_episode_max_count: usize,
    ) -> Result<Self> {
        ensure!(
            reject_episode_max_count > 0,
            "reject_episode_max_count must be positive"
        );
        let machine_relative =
            CanonicalRelativeEvidencePath::parse("machine_relative_path", machine_relative_path)
                .map_err(anyhow::Error::msg)?;
        let observation_relative = CanonicalRelativeEvidencePath::parse(
            "observation_relative_path",
            observation_relative_path,
        )
        .map_err(anyhow::Error::msg)?;
        ensure_path_topology([&machine_relative, &observation_relative])?;
        let catalog = CatalogDirectory::open_writer(catalog_directory)?;
        let mut machine = catalog.open_stream(&machine_relative)?;
        let observation = catalog.open_stream(&observation_relative)?;
        ensure_distinct_files(&machine, &observation)?;
        ensure!(
            machine.metadata()?.len() == 0 && observation.metadata()?.len() == 0,
            "offline current-evidence streams must be fresh"
        );
        let recovery = validate_stream(
            &mut machine,
            KnownSink::Machine,
            recovery_evidence_max_bytes,
        )?
        .startup_recovery;
        ensure!(
            recovery.reservation.is_empty()
                && recovery.settlement.is_empty()
                && recovery.booking.is_empty(),
            "fresh offline current-evidence stream produced recovery state"
        );
        machine.seek(SeekFrom::End(0))?;
        Ok(Self {
            recorder: Arc::new(DecisionEvidenceRecorder::from_files(
                machine,
                observation,
                Some(catalog),
                None,
                recovery_evidence_max_bytes,
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
        let machine_relative = CanonicalRelativeEvidencePath::parse(
            "machine_relative_path",
            &config.machine_relative_path,
        )
        .map_err(anyhow::Error::msg)?;
        let observation_relative = CanonicalRelativeEvidencePath::parse(
            "observation_relative_path",
            &config.observation_relative_path,
        )
        .map_err(anyhow::Error::msg)?;
        let retired_paths = config
            .retired_relative_paths
            .iter()
            .map(|retired| {
                CanonicalRelativeEvidencePath::parse("retired_relative_paths", retired)
                    .map_err(anyhow::Error::msg)
            })
            .collect::<Result<Vec<_>>>()?;
        ensure_path_topology(
            std::iter::once(&machine_relative)
                .chain(std::iter::once(&observation_relative))
                .chain(retired_paths.iter()),
        )?;
        let recovery_evidence_max_bytes =
            PositiveFiniteEvidenceReadCap::new(config.recovery_evidence_max_bytes)
                .map_err(anyhow::Error::msg)?;
        let catalog =
            CatalogDirectory::open_writer(Path::new(&loaded.root.persistence.catalog_directory))?;
        for retired_relative in &retired_paths {
            catalog.ensure_retired_absent(retired_relative)?;
        }

        let mut machine = catalog
            .open_stream(&machine_relative)
            .with_context(|| format!("open machine evidence `{}`", machine_relative.as_str()))?;
        let startup_recovery = validate_stream(
            &mut machine,
            KnownSink::Machine,
            recovery_evidence_max_bytes,
        )?
        .startup_recovery;
        machine.seek(SeekFrom::End(0))?;
        let mut observation = catalog
            .open_stream(&observation_relative)
            .with_context(|| {
                format!(
                    "open observation evidence `{}`",
                    observation_relative.as_str()
                )
            })?;
        ensure_distinct_files(&machine, &observation)?;
        let observation_poison = match validate_stream(
            &mut observation,
            KnownSink::Observation,
            recovery_evidence_max_bytes,
        ) {
            Ok(_) => None,
            Err(error) => {
                let cause: Arc<str> = Arc::from(format!("{error:#}"));
                log::error!("retained observation evidence is poisoned: {cause}");
                Some(PoisonCause::StartupContentInvalid { cause })
            }
        };
        observation.seek(SeekFrom::End(0))?;

        Ok(Self {
            recorder: Arc::new(DecisionEvidenceRecorder::from_files(
                machine,
                observation,
                Some(catalog),
                observation_poison,
                recovery_evidence_max_bytes,
                config.reject_episode_max_count,
            )),
            reservation_recovery: Arc::new(startup_recovery.reservation),
            settlement_recovery: Arc::new(startup_recovery.settlement),
            booking_recovery: Arc::new(startup_recovery.booking),
        })
    }

    #[must_use]
    pub fn recorder(&self) -> Arc<DecisionEvidenceRecorder> {
        Arc::clone(&self.recorder)
    }

    #[must_use]
    pub fn reservation_recovery(&self) -> Arc<ReservationRecoveryFacts> {
        Arc::clone(&self.reservation_recovery)
    }

    #[must_use]
    pub fn settlement_recovery(&self) -> Arc<SettlementRecoveryFacts> {
        Arc::clone(&self.settlement_recovery)
    }

    #[must_use]
    pub fn booking_recovery(&self) -> Arc<BookingRecoveryFacts> {
        Arc::clone(&self.booking_recovery)
    }

    #[must_use]
    pub fn observation_stream_status(&self) -> ObservationStreamStatus {
        self.recorder.observation_stream_status()
    }
}

fn ensure_path_topology<'a>(
    configured: impl IntoIterator<Item = &'a CanonicalRelativeEvidencePath>,
) -> Result<()> {
    let configured = configured.into_iter().collect::<Vec<_>>();
    for (index, left) in configured.iter().enumerate() {
        for right in &configured[index + 1..] {
            ensure!(
                left != right,
                "decision-evidence paths must be distinct: `{}`",
                left.as_str()
            );
            ensure!(
                !left.is_ancestor_of(right) && !right.is_ancestor_of(left),
                "decision-evidence paths must not be ancestors of one another: `{}` and `{}`",
                left.as_str(),
                right.as_str()
            );
        }
    }
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
    anyhow::bail!("decision-evidence runtime is unsupported on non-Unix targets")
}
