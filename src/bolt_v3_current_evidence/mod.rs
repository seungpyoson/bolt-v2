mod codec;
pub mod contract_generator;
mod facts;
pub(crate) mod generated_contract;
mod path;
mod path_authority;
mod reader;
mod record;
mod runtime;

pub use facts::*;
pub(crate) use path::validate_relative_path;
pub use reader::{ShadowPnlEvent, read_current_evidence_facts, read_shadow_pnl_events};
pub use record::{
    AppendReceipt, CommitPhase, DecisionEvidenceRecorder, NonBlockingRecordOutcome,
    ObservationRecordOutcome, PoisonCause, RecordFailure,
};
#[cfg(feature = "offline-current-evidence")]
pub use runtime::OfflineDecisionEvidenceRuntime;
pub use runtime::{DecisionEvidenceRuntime, ObservationStreamStatus};

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
