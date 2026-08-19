mod capacity;
mod codec;
pub mod contract_generator;
mod facts;
pub(crate) mod generated_contract;
mod handles;
mod path;
mod path_authority;
mod reader;
mod realized_volatility;
mod record;
mod runtime;

pub use capacity::PositiveFiniteEvidenceReadCap;
pub use facts::*;
#[cfg(any(test, feature = "test-current-evidence-inspection"))]
#[doc(hidden)]
pub use generated_contract::KnownPurpose as CurrentEvidenceTestPurpose;
pub use handles::{
    BasketAdmissionEvidence, EdgeTakerEvidence, MakerEvidence, OrderExecutionEvidence,
    OrderRejectObserverEvidence, StrategyEvidenceHandles, SubmitAdmissionEvidence,
};
pub(crate) use path::CanonicalRelativeEvidencePath;
pub(crate) use path_authority::CatalogDirectory;
#[cfg(feature = "test-current-evidence-inspection")]
#[doc(hidden)]
pub use reader::read_current_evidence_facts;
pub use reader::{
    BacktestRunGuardEvent, CurrentEvidenceStream, RecordedBacktestRunGuardEvent, ShadowPnlEvent,
    read_backtest_run_guard_events, read_shadow_pnl_events,
};
pub(crate) use realized_volatility::source_diagnostic_fact as realized_vol_diagnostic_fact;
#[cfg(not(any(test, feature = "test-current-evidence-inspection")))]
pub(crate) use record::DecisionEvidenceRecorder;
#[cfg(any(test, feature = "test-current-evidence-inspection"))]
#[doc(hidden)]
pub use record::DecisionEvidenceRecorder;
pub(crate) use record::DecisionEvidenceStatusView;
pub use record::{
    AppendReceipt, CommitPhase, CommittedAdmission, CommittedSettlement, NonBlockingRecordOutcome,
    ObservationRecordOutcome, ObservationStreamStatus, PoisonCause, RecordFailure,
};
#[cfg(feature = "test-current-evidence-inspection")]
#[doc(hidden)]
pub use record::{DecisionEvidenceIntegrationTestControl, DecisionEvidenceIntegrationTestFault};
pub use runtime::DecisionEvidenceRuntime;
#[cfg(feature = "offline-current-evidence")]
pub use runtime::OfflineDecisionEvidenceRuntime;

/// Proves prefix containment and probes an existing catalog through one descriptor-relative authority.
pub fn prestart_catalog_check(
    required_prefix: &std::path::Path,
    catalog: &std::path::Path,
) -> anyhow::Result<u64> {
    path_authority::CatalogDirectory::open_under_prefix(required_prefix, catalog)?
        .prestart_probe_and_available_bytes()
}

#[cfg(test)]
pub(crate) fn prepare_test_generation(loaded: &crate::bolt_v3_config::LoadedBoltV3Config) {
    let catalog = std::path::Path::new(&loaded.root.persistence.catalog_directory);
    let evidence = &loaded.root.persistence.decision_evidence;
    for relative in [
        &evidence.machine_relative_path,
        &evidence.observation_relative_path,
    ] {
        let stream = catalog.join(relative);
        std::fs::create_dir_all(
            stream
                .parent()
                .expect("test evidence stream path must have a parent"),
        )
        .expect("test evidence generation directory must create");
    }
}

pub(crate) fn settlement_kind() -> &'static str {
    generated_contract::descriptor_for_identity(generated_contract::KnownIdentity::SettlementV1)
        .kind
}
