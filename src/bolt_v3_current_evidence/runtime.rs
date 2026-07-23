use std::{
    collections::BTreeSet,
    fs::File,
    io::{Seek, SeekFrom},
    path::Path,
    sync::Arc,
};

use anyhow::{Context, Result, ensure};

use crate::bolt_v3_config::LoadedBoltV3Config;

use super::{
    facts::{BookingRecoveryFacts, ReservationRecoveryFacts, SettlementRecoveryFacts},
    generated_contract::KnownSink,
    path_authority::CatalogDirectory,
    reader::validate_stream,
    record::{DecisionEvidenceRecorder, ObservationStreamStatus, PoisonCause},
    validate_relative_path,
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
        let recovery = validate_stream(&mut machine, KnownSink::Machine, 0)?.startup_recovery;
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
                None,
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
        let machine_relative =
            configured_relative("machine_relative_path", &config.machine_relative_path)?;
        let observation_relative = configured_relative(
            "observation_relative_path",
            &config.observation_relative_path,
        )?;
        ensure!(
            machine_relative != observation_relative,
            "decision-evidence paths must be distinct"
        );

        let mut configured = BTreeSet::from([machine_relative, observation_relative]);
        let catalog =
            CatalogDirectory::open(Path::new(&loaded.root.persistence.catalog_directory))?;
        for retired in &config.retired_relative_paths {
            let retired_relative = configured_relative("retired_relative_paths", retired)?;
            ensure!(
                configured.insert(retired_relative),
                "decision-evidence paths must be distinct: `{}`",
                retired_relative
            );
            catalog.ensure_retired_absent(retired_relative)?;
        }

        let mut machine = catalog
            .open_stream(machine_relative)
            .with_context(|| format!("open machine evidence `{machine_relative}`"))?;
        let startup_recovery = validate_stream(
            &mut machine,
            KnownSink::Machine,
            config.recovery_evidence_max_bytes,
        )?
        .startup_recovery;
        machine.seek(SeekFrom::End(0))?;
        let mut observation = catalog
            .open_stream(observation_relative)
            .with_context(|| format!("open observation evidence `{observation_relative}`"))?;
        ensure_distinct_files(&machine, &observation)?;
        let observation_poison = match validate_stream(
            &mut observation,
            KnownSink::Observation,
            config.recovery_evidence_max_bytes,
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
                observation_poison,
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

fn configured_relative<'a>(field: &str, raw: &'a str) -> Result<&'a str> {
    validate_relative_path(field, raw).map_err(anyhow::Error::msg)?;
    Ok(raw.trim())
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
