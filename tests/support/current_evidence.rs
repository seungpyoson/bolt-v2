use std::{ops::Deref, path::PathBuf, sync::Arc};

use bolt_v2::{
    bolt_v3_config::{LoadedBoltV3Config, load_bolt_v3_config},
    bolt_v3_current_evidence::{
        AdmissionDecisionOutcome, AdmissionDetails, CurrentEvidenceTestPurpose, CurrentFact,
        DecisionEvidenceRecorder, DecisionEvidenceRuntime, EntrySkipFact, LossGovernorHaltFact,
        OrderIntentDetails, OrderRejectFact, PositiveFiniteEvidenceReadCap,
        RequoteThrottleObservationFact, ReservationAttribution, VenueTruthCaptureFailureFact,
        VenueTruthDivergenceFact, read_current_evidence_facts,
    },
    bolt_v3_submit_admission::BoltV3SubmitIntentKind,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedOrderIntent {
    pub details: OrderIntentDetails,
}

impl Deref for RecordedOrderIntent {
    type Target = OrderIntentDetails;

    fn deref(&self) -> &Self::Target {
        &self.details
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedAdmissionDecision {
    pub intent_kind: BoltV3SubmitIntentKind,
    pub outcome: AdmissionDecisionOutcome,
    pub details: AdmissionDetails,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordedBasketAdmissionOutcome {
    Granted,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedBasketAdmissionDecision {
    pub outcome: RecordedBasketAdmissionOutcome,
    pub details: bolt_v2::bolt_v3_current_evidence::BasketAdmissionDetails,
}

impl Deref for RecordedBasketAdmissionDecision {
    type Target = bolt_v2::bolt_v3_current_evidence::BasketAdmissionDetails;

    fn deref(&self) -> &Self::Target {
        &self.details
    }
}

impl Deref for RecordedAdmissionDecision {
    type Target = AdmissionDetails;

    fn deref(&self) -> &Self::Target {
        &self.details
    }
}

#[derive(Debug)]
pub struct RecordingDecisionEvidenceWriter {
    _directory: tempfile::TempDir,
    runtime: DecisionEvidenceRuntime,
    machine_path: PathBuf,
    observation_path: PathBuf,
    read_cap: PositiveFiniteEvidenceReadCap,
}

impl Default for RecordingDecisionEvidenceWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingDecisionEvidenceWriter {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().expect("decision-evidence test directory must create");
        let catalog_directory = std::fs::canonicalize(directory.path())
            .expect("decision-evidence test directory must canonicalize");
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/bolt_v3/root.toml");
        let mut loaded = load_bolt_v3_config(&fixture).expect("fixture config must load");
        loaded.root.persistence.catalog_directory = catalog_directory.display().to_string();
        let evidence = &loaded.root.persistence.decision_evidence;
        let machine_path = catalog_directory.join(&evidence.machine_relative_path);
        let observation_path = catalog_directory.join(&evidence.observation_relative_path);
        let read_cap = PositiveFiniteEvidenceReadCap::new(evidence.recovery_evidence_max_bytes)
            .expect("fixture recovery evidence cap must be positive and finite");
        prepare_current_evidence_generation(&loaded);
        let runtime = DecisionEvidenceRuntime::open(&loaded)
            .expect("current decision-evidence runtime must open");
        Self {
            _directory: directory,
            runtime,
            machine_path,
            observation_path,
            read_cap,
        }
    }

    pub fn recorder(&self) -> Arc<DecisionEvidenceRecorder> {
        self.runtime.recorder()
    }

    pub fn fail_machine_writes(&self) {
        self.runtime.recorder().fail_machine_writes_for_test();
    }

    pub fn fail_purpose_on_attempt(&self, purpose: CurrentEvidenceTestPurpose, attempt: usize) {
        self.runtime
            .recorder()
            .fail_purpose_on_attempt_for_test(purpose, attempt);
    }

    pub fn facts(&self) -> Vec<CurrentFact> {
        let mut facts = read_current_evidence_facts(&self.machine_path, self.read_cap)
            .expect("machine evidence must decode");
        facts.extend(
            read_current_evidence_facts(&self.observation_path, self.read_cap)
                .expect("observation evidence must decode"),
        );
        facts
    }

    pub fn records(&self) -> Vec<RecordedOrderIntent> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                CurrentFact::EntryOrderIntent(fact) => Some(RecordedOrderIntent {
                    details: fact.details,
                }),
                CurrentFact::RiskReducingExitOrderIntent(fact) => Some(RecordedOrderIntent {
                    details: fact.details,
                }),
                _ => None,
            })
            .collect()
    }

    pub fn admission_decisions(&self) -> Vec<RecordedAdmissionDecision> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                CurrentFact::AdmittedEntryAdmission(fact) => Some(RecordedAdmissionDecision {
                    intent_kind: BoltV3SubmitIntentKind::Entry,
                    outcome: AdmissionDecisionOutcome::Admitted,
                    details: fact.details,
                }),
                CurrentFact::RejectedEntryAdmission(fact) => Some(RecordedAdmissionDecision {
                    intent_kind: BoltV3SubmitIntentKind::Entry,
                    outcome: AdmissionDecisionOutcome::Rejected(fact.reason),
                    details: fact.details,
                }),
                CurrentFact::RiskReducingExitAdmission(fact) => Some(RecordedAdmissionDecision {
                    intent_kind: BoltV3SubmitIntentKind::RiskReducingExit,
                    outcome: fact.outcome,
                    details: fact.details,
                }),
                CurrentFact::ForcedReductionAdmission(fact) => Some(RecordedAdmissionDecision {
                    intent_kind: BoltV3SubmitIntentKind::KillSwitchForcedReduction,
                    outcome: fact.outcome,
                    details: fact.details,
                }),
                _ => None,
            })
            .collect()
    }

    pub fn entry_skips(&self) -> Vec<EntrySkipFact> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                CurrentFact::EntrySkipObservation(fact) => Some(*fact),
                _ => None,
            })
            .collect()
    }

    pub fn basket_admission_decisions(&self) -> Vec<RecordedBasketAdmissionDecision> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                CurrentFact::BasketAdmissionGranted(fact) => {
                    Some(RecordedBasketAdmissionDecision {
                        outcome: RecordedBasketAdmissionOutcome::Granted,
                        details: fact.details,
                    })
                }
                CurrentFact::BasketAdmissionRejected(fact) => {
                    Some(RecordedBasketAdmissionDecision {
                        outcome: RecordedBasketAdmissionOutcome::Rejected,
                        details: fact.details,
                    })
                }
                _ => None,
            })
            .collect()
    }

    pub fn reservation_attributions(&self) -> Vec<ReservationAttribution> {
        self.facts()
            .into_iter()
            .flat_map(|fact| match fact {
                CurrentFact::AdmittedEntryAdmission(fact) => fact.reservation.into_iter().collect(),
                CurrentFact::BasketAdmissionGranted(fact) => fact
                    .admitted_legs
                    .into_iter()
                    .filter_map(|leg| leg.reservation)
                    .collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    pub fn order_rejects(&self) -> Vec<OrderRejectFact> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                CurrentFact::OrderReject(fact) => Some(*fact),
                _ => None,
            })
            .collect()
    }

    pub fn loss_governor_halts(&self) -> Vec<LossGovernorHaltFact> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                CurrentFact::LossGovernorHalt(fact) => Some(fact),
                _ => None,
            })
            .collect()
    }

    pub fn requote_throttles(&self) -> Vec<RequoteThrottleObservationFact> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                CurrentFact::RequoteThrottleObservation(fact) => Some(fact),
                _ => None,
            })
            .collect()
    }

    pub fn venue_truth_capture_failures(&self) -> Vec<VenueTruthCaptureFailureFact> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                CurrentFact::VenueTruthCaptureFailure(fact) => Some(fact),
                _ => None,
            })
            .collect()
    }

    pub fn venue_truth_divergences(&self) -> Vec<VenueTruthDivergenceFact> {
        self.facts()
            .into_iter()
            .filter_map(|fact| match fact {
                CurrentFact::VenueTruthDivergence(fact) => Some(fact),
                _ => None,
            })
            .collect()
    }
}

pub fn prepare_current_evidence_generation(loaded: &LoadedBoltV3Config) {
    let catalog = PathBuf::from(&loaded.root.persistence.catalog_directory);
    let evidence = &loaded.root.persistence.decision_evidence;
    for relative_path in [
        &evidence.machine_relative_path,
        &evidence.observation_relative_path,
    ] {
        let path = catalog.join(relative_path);
        std::fs::create_dir_all(
            path.parent()
                .expect("current evidence stream path must have a parent"),
        )
        .expect("current decision-evidence generation directory must create");
    }
}

pub fn recording_evidence() -> Arc<DecisionEvidenceRecorder> {
    Arc::new(DecisionEvidenceRecorder::recording_from_files_for_test(
        tempfile::tempfile().expect("test machine evidence sink must open"),
        tempfile::tempfile().expect("test observation evidence sink must open"),
    ))
}
