use std::{
    collections::BTreeSet,
    str::FromStr,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use nautilus_backtest::engine::BacktestEngine;
use nautilus_common::enums::Environment;
use nautilus_core::UnixNanos;
use nautilus_event_store::{
    DataCursorSnapshot, DataMarkerClass, DataMarkerConfig, EncodedPayload, EncoderRegistry,
    EventStoreConfig, EventStoreEntry, EventStoreLifecycle, EventStoreLifecycleOptions,
    EventStoreReader, MarkerBackend, MarkerGap, MarkerManifest, MarkerVerifier, RedbBackend,
    RedbMarkerBackend, RegisteredComponents, ScanDirection, StreamDictEntry, Verifier,
    default_registry,
};
use nautilus_model::data::{InstrumentClose, OrderBookDelta};
use nautilus_model::identifiers::InstrumentId;
use ustr::Ustr;

use crate::{run_manifest::BacktestingRunManifest, runner::NtBacktestNodeRun};

const INSTRUMENT_CLOSE_PAYLOAD_TYPE: &str = "InstrumentClose";
static EVIDENCE_CAPTURE_LOCK: Mutex<()> = Mutex::new(());

fn issue_789_data_marker_config() -> DataMarkerConfig {
    DataMarkerConfig {
        classes: vec![DataMarkerClass::BookDeltas],
        // #789 needs entry-bound cursors, not time-based snapshots. Disabling
        // safety flushes removes the otherwise ambiguous same-sequence,
        // same-timestamp post-submit boundary without recording every update.
        safety_flush_interval: Duration::MAX,
        ..Default::default()
    }
}

/// Ordered NautilusTrader evidence captured during one BTE run.
#[derive(Debug)]
pub(crate) struct ExecutionEvidence {
    pub(crate) entries: Vec<EventStoreEntry>,
    pub(crate) marker_manifest: MarkerManifest,
    pub(crate) marker_snapshots: Vec<DataCursorSnapshot>,
    pub(crate) marker_gaps: Vec<MarkerGap>,
    pub(crate) stream_dictionary: Vec<StreamDictEntry>,
}

impl ExecutionEvidence {
    pub(crate) fn book_delta_count_at(
        &self,
        event_seq: u64,
        event_ts_init: UnixNanos,
        instrument_id: &str,
        catalog_deltas: &[OrderBookDelta],
    ) -> Result<usize> {
        let slot = self
            .stream_dictionary
            .iter()
            .find(|entry| entry.identifier == instrument_id)
            .with_context(|| format!("marker dictionary is missing {instrument_id}"))?
            .slot;
        let delta_count = snapshot_book_delta_count_at(
            &self.marker_snapshots,
            slot,
            event_seq,
            event_ts_init,
            instrument_id,
        )?;
        ensure!(
            delta_count <= catalog_deltas.len(),
            "#789 event-bound book cursor {delta_count} exceeds hash-bound catalog length {} for {instrument_id}",
            catalog_deltas.len()
        );
        let expected_id = InstrumentId::from_str(instrument_id)
            .map_err(|error| anyhow::anyhow!(error))
            .with_context(|| format!("parse executable-book instrument id {instrument_id:?}"))?;
        ensure!(
            catalog_deltas
                .iter()
                .all(|delta| delta.instrument_id == expected_id),
            "hash-bound catalog mixes instruments in the {instrument_id} stream"
        );
        Ok(delta_count)
    }

    pub(crate) fn ensure_issue_789_causal_surface(
        &self,
        manifest: &BacktestingRunManifest,
    ) -> Result<()> {
        ensure!(!self.entries.is_empty(), "#789 event store is empty");
        for (index, entry) in self.entries.iter().enumerate() {
            ensure!(
                entry.seq == index as u64 + 1,
                "#789 event-store sequence is not contiguous at index {index}: got {}",
                entry.seq
            );
        }
        ensure!(
            self.marker_manifest.is_sealed(),
            "#789 data-marker sidecar was not sealed"
        );
        ensure!(
            !self.marker_manifest.high_fidelity,
            "#789 uses the synchronous entry-bound cursor, not unbounded per-record capture"
        );
        ensure!(
            self.marker_gaps.is_empty(),
            "#789 data-marker capture contains gaps: {:?}",
            self.marker_gaps
        );

        let payload_types = self
            .entries
            .iter()
            .map(|entry| entry.payload_type.as_str())
            .collect::<BTreeSet<_>>();
        for required in [
            "SubmitOrder",
            "OrderFilled",
            "AccountState",
            INSTRUMENT_CLOSE_PAYLOAD_TYPE,
        ] {
            ensure!(
                payload_types.contains(required),
                "#789 event store is missing required {required} evidence"
            );
        }
        ensure!(
            payload_types
                .iter()
                .any(|payload_type| payload_type.starts_with("Position")),
            "#789 event store is missing position-effect evidence"
        );

        let book_instruments = executable_book_instruments(manifest)?;
        for instrument_id in book_instruments {
            let slot = self
                .stream_dictionary
                .iter()
                .find(|entry| entry.identifier == instrument_id)
                .with_context(|| format!("#789 marker dictionary is missing {instrument_id}"))?
                .slot;
            ensure!(
                self.marker_snapshots
                    .iter()
                    .flat_map(|snapshot| snapshot.advanced.iter())
                    .any(|cursor| cursor.slot == slot),
                "#789 event-bound book cursor is missing for {instrument_id}"
            );
        }
        ensure!(
            !self.marker_snapshots.is_empty(),
            "#789 data-marker sidecar has no event-bound cursor snapshots"
        );
        Ok(())
    }
}

fn snapshot_book_delta_count_at(
    snapshots: &[DataCursorSnapshot],
    slot: u32,
    event_seq: u64,
    event_ts_init: UnixNanos,
    instrument_id: &str,
) -> Result<usize> {
    let mut count = None;
    let mut synchronous_snapshot_seen = false;
    for snapshot in snapshots {
        let before_submit = snapshot.event_seq_before < event_seq;
        let synchronous_submit_boundary =
            snapshot.event_seq_before == event_seq && snapshot.ts_init == event_ts_init;
        if synchronous_submit_boundary {
            ensure!(
                !synchronous_snapshot_seen,
                "#789 has multiple synchronous cursor snapshots at SubmitOrder sequence {event_seq}"
            );
            synchronous_snapshot_seen = true;
        }
        if (before_submit || synchronous_submit_boundary)
            && let Some(cursor) = snapshot.advanced.iter().find(|cursor| cursor.slot == slot)
        {
            count = Some(cursor.count);
        }
    }
    let count = count.with_context(|| {
        format!(
            "no executable-book cursor precedes SubmitOrder sequence {event_seq} for {instrument_id}"
        )
    })?;
    usize::try_from(count).context("book-delta cursor does not fit usize")
}

fn ensure_marker_integrity(backend: &dyn MarkerBackend, high_watermark: u64) -> Result<()> {
    let report = MarkerVerifier::scan(backend, high_watermark)
        .context("verify #789 data-marker sidecar integrity")?;
    ensure!(
        report.is_clean(),
        "#789 data-marker sidecar failed integrity verification: {:?}",
        report.findings
    );
    Ok(())
}

pub(crate) struct ExecutionEvidenceCapture {
    _capture_guard: MutexGuard<'static, ()>,
    tempdir: tempfile::TempDir,
    lifecycle: EventStoreLifecycle,
    instance_id: String,
    run_id: String,
    clock: std::rc::Rc<std::cell::RefCell<dyn nautilus_common::clock::Clock>>,
}

impl ExecutionEvidenceCapture {
    pub(crate) fn start(
        engine: &BacktestEngine,
        manifest: &BacktestingRunManifest,
    ) -> Result<Self> {
        let capture_guard = EVIDENCE_CAPTURE_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("#789 evidence capture lock was poisoned"))?;
        let tempdir = tempfile::TempDir::new().context("create #789 event-store directory")?;
        let config = EventStoreConfig {
            base_dir: tempdir.path().to_path_buf(),
            identity: nautilus_event_store::RunIdentity {
                binary_hash: manifest.target_bolt_v2_ref.clone(),
                crate_versions: manifest.resolved_nt_version.clone(),
                config_hash: manifest.manifest_hash(),
                ..Default::default()
            },
            data_markers: Some(issue_789_data_marker_config()),
            ..Default::default()
        };

        let mut registry = default_registry();
        register_instrument_close(&mut registry);
        let options = EventStoreLifecycleOptions::new().with_encoder_registry(registry);
        let instance_id = engine.instance_id();
        let clock = engine.kernel().clock();
        let mut lifecycle = EventStoreLifecycle::boot_with_options(
            Some(config),
            instance_id,
            std::rc::Rc::clone(&clock),
            options,
        )
        .context("boot #789 NT event store")?;
        lifecycle
            .open(
                instance_id,
                &RegisteredComponents::default(),
                Environment::Backtest,
            )
            .context("open #789 NT event-store run")?;
        let run_id = lifecycle
            .run_id()
            .context("NT event store opened without a run id")?
            .to_string();

        Ok(Self {
            _capture_guard: capture_guard,
            tempdir,
            lifecycle,
            instance_id: instance_id.to_string(),
            run_id,
            clock,
        })
    }

    pub(crate) fn finish(mut self) -> Result<ExecutionEvidence> {
        ensure!(
            !self.lifecycle.is_halted(),
            "#789 NT event-store writer fail-stopped"
        );
        let ts_init = self.clock.borrow().timestamp_ns();
        self.lifecycle.seal(ts_init);
        ensure!(
            !self.lifecycle.is_halted(),
            "#789 NT event-store writer halted while sealing"
        );

        let verification =
            Verifier::open_redb(self.tempdir.path(), &self.instance_id, &self.run_id)
                .context("open sealed #789 NT event store for verification")?
                .verify()
                .context("verify sealed #789 NT event store")?;
        ensure!(
            verification.is_clean(),
            "#789 NT event store failed integrity verification: {:?}",
            verification.findings
        );

        let backend =
            RedbBackend::open_sealed(self.tempdir.path(), &self.instance_id, &self.run_id)
                .context("open sealed #789 NT event store")?;
        let reader = EventStoreReader::new(backend);
        let high_watermark = reader
            .high_watermark()
            .context("read #789 event-store high watermark")?;
        let entries = reader
            .scan_range(1, high_watermark, ScanDirection::Forward)
            .collect::<Result<Vec<_>, _>>()
            .context("read sealed #789 event-store entries")?;

        let marker_path = self
            .tempdir
            .path()
            .join(&self.instance_id)
            .join(format!("{}.markers.redb", self.run_id));
        let marker_backend = RedbMarkerBackend::open_read_only_file(marker_path)
            .context("open sealed #789 data-marker sidecar")?;
        ensure_marker_integrity(&marker_backend, high_watermark)?;
        let marker_manifest = marker_backend.manifest().context("read marker manifest")?;
        let marker_snapshots = marker_backend
            .scan_snapshots()
            .context("read marker snapshots")?;
        let marker_gaps = marker_backend.scan_gaps().context("read marker gaps")?;
        let stream_dictionary = marker_backend
            .scan_dict()
            .context("read marker stream dictionary")?;

        Ok(ExecutionEvidence {
            entries,
            marker_manifest,
            marker_snapshots,
            marker_gaps,
            stream_dictionary,
        })
    }
}

fn executable_book_instruments(manifest: &BacktestingRunManifest) -> Result<BTreeSet<String>> {
    manifest
        .catalog_inputs
        .iter()
        .filter(|input| input.data_type == "OrderBookDelta")
        .filter_map(|input| {
            let instrument_id = InstrumentId::from_str(&input.nt_instrument_id)
                .map_err(|error| anyhow::anyhow!("{error}"));
            match instrument_id {
                Ok(instrument_id) if instrument_id.venue.as_str() == manifest.venue.nt_venue => {
                    Some(Ok(input.nt_instrument_id.clone()))
                }
                Ok(_) => None,
                Err(error) => Some(Err(error).with_context(|| {
                    format!(
                        "parse executable book instrument id {:?}",
                        input.nt_instrument_id
                    )
                })),
            }
        })
        .collect()
}

fn register_instrument_close(registry: &mut EncoderRegistry) {
    registry.register::<InstrumentClose, _>(Ustr::from(INSTRUMENT_CLOSE_PAYLOAD_TYPE), |close| {
        rmp_serde::to_vec_named(close)
            .map(Bytes::from)
            .map(EncodedPayload::without_indices)
            .map_err(|error| nautilus_event_store::EncodeError::Serialize(error.to_string()))
    });
}

pub(crate) fn run_nt_backtest_node_with_evidence(
    manifest: &BacktestingRunManifest,
) -> Result<(NtBacktestNodeRun, ExecutionEvidence)> {
    crate::runner::run_nt_backtest_node_capturing_evidence(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_core::UnixNanos;
    use nautilus_event_store::{
        DataClass, DataCursorSnapshot, MarkerManifest, MemoryMarkerBackend, RunStatus, StreamCursor,
    };

    const INSTRUMENT_ID: &str = "YES.POLYMARKET";

    fn snapshot(
        marker_seq: u64,
        event_seq_before: u64,
        ts_init: u64,
        count: u64,
    ) -> DataCursorSnapshot {
        DataCursorSnapshot {
            marker_seq,
            event_seq_before,
            ts_init: UnixNanos::from(ts_init),
            advanced: vec![StreamCursor {
                slot: 0,
                ts_init_hi: UnixNanos::from(ts_init),
                count,
            }],
        }
    }

    #[test]
    fn executable_book_cursor_excludes_later_same_boundary_snapshot() {
        let snapshots = vec![
            snapshot(1, 4, 100, 1),
            snapshot(2, 5, 200, 2),
            snapshot(3, 5, 500, 3),
        ];
        let count =
            snapshot_book_delta_count_at(&snapshots, 0, 5, UnixNanos::from(200), INSTRUMENT_ID)
                .expect("synchronous SubmitOrder cursor");
        assert_eq!(count, 2);
    }

    #[test]
    fn executable_book_cursor_uses_prior_snapshot_when_submit_did_not_advance_data() {
        let snapshots = vec![snapshot(1, 4, 100, 1), snapshot(2, 5, 500, 2)];
        let count =
            snapshot_book_delta_count_at(&snapshots, 0, 5, UnixNanos::from(200), INSTRUMENT_ID)
                .expect("prior event-bound cursor");
        assert_eq!(count, 1);
    }

    #[test]
    fn executable_book_cursor_rejects_missing_stream_marker() {
        let error = snapshot_book_delta_count_at(
            &[snapshot(1, 4, 100, 1)],
            1,
            5,
            UnixNanos::from(200),
            INSTRUMENT_ID,
        )
        .expect_err("missing stream cursor must fail closed");
        assert!(error.to_string().contains("no executable-book cursor"));
    }

    #[test]
    fn executable_book_cursor_rejects_ambiguous_synchronous_snapshots() {
        let snapshots = vec![snapshot(1, 5, 200, 1), snapshot(2, 5, 200, 2)];
        let error =
            snapshot_book_delta_count_at(&snapshots, 0, 5, UnixNanos::from(200), INSTRUMENT_ID)
                .expect_err("ambiguous synchronous snapshots must fail closed");
        assert!(error.to_string().contains("multiple synchronous"));
    }

    #[test]
    fn marker_integrity_rejects_tampered_stored_hash() {
        let mut backend = MemoryMarkerBackend::new();
        backend
            .open_run(MarkerManifest {
                run_id: "issue-789-marker-integrity".to_string(),
                enabled_classes: vec![DataClass::BookDeltas],
                high_fidelity: false,
                snapshot_count: 0,
                hifi_count: 0,
                gap_count: 0,
                dict_count: 0,
                status: RunStatus::Running,
            })
            .expect("open marker backend");
        let snapshot = DataCursorSnapshot {
            marker_seq: 1,
            event_seq_before: 1,
            ts_init: UnixNanos::from(1),
            advanced: vec![StreamCursor {
                slot: 0,
                ts_init_hi: UnixNanos::from(1),
                count: 1,
            }],
        };
        backend
            .append_snapshot(&snapshot, [0xAA; 32])
            .expect("append tampered-hash snapshot");
        backend.seal(RunStatus::Ended).expect("seal marker backend");

        let error = ensure_marker_integrity(&backend, 1)
            .expect_err("stored marker hash mismatch must fail closed");
        assert!(error.to_string().contains("integrity verification"));
    }

    #[test]
    fn evidence_capture_disables_ambiguous_time_based_marker_flushes() {
        let config = issue_789_data_marker_config();
        assert_eq!(config.safety_flush_interval, Duration::MAX);
        assert!(config.high_fidelity.is_empty());
    }
}
