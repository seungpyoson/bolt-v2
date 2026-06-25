use rust_decimal::Decimal;

use crate::bolt_v3_risk_reservation_substrate::{
    contracts::{
        ActiveDescriptorView, PolicyApproval, PreparedEpochAttestation, PreparedEpochDescriptor,
        PreparedPolicyEpoch, SafetyEnvelopeInvariant, SafetyPolicyEnvelope,
    },
    risk_classifier::{RiskClassificationError, RiskClassifier},
    state_owner::{PolicyEpochSnapshot, RiskStateMutationError, RiskStateOwner},
};

pub use crate::bolt_v3_risk_reservation_substrate::state_owner::PolicyEpochAlertReason;

#[derive(Debug, Clone)]
pub struct EpochManager {
    owner: RiskStateOwner,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedEpochCutover {
    prepared_epoch: PreparedPolicyEpoch,
    envelope: SafetyPolicyEnvelope,
    drain_report: VenueEventDrainReport,
    post_cutover_admission_state: PostCutoverAdmissionState,
}

impl PreparedEpochCutover {
    pub fn prepared_epoch(&self) -> &PreparedPolicyEpoch {
        &self.prepared_epoch
    }

    pub fn envelope(&self) -> &SafetyPolicyEnvelope {
        &self.envelope
    }

    pub const fn drain_report(&self) -> VenueEventDrainReport {
        self.drain_report
    }

    pub const fn post_cutover_admission_state(&self) -> PostCutoverAdmissionState {
        self.post_cutover_admission_state
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostCutoverAdmissionState {
    pub current_exposure_compliant: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyEpochActivation {
    pub prior_epoch_id: Option<String>,
    pub active_epoch_id: String,
    pub risk_state_version: crate::bolt_v3_risk_reservation_substrate::contracts::RiskStateVersion,
    pub risk_increasing_admission_enabled: bool,
    pub safety_action_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VenueEventDrainReport {
    pub drained_event_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueEventDrainError {
    DrainFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedEpochRevaluationInput<'a> {
    pub prepared_epoch: &'a PreparedPolicyEpoch,
    pub envelope: &'a SafetyPolicyEnvelope,
    pub drain_report: VenueEventDrainReport,
    pub current_policy_state: PolicyEpochSnapshot,
}

pub trait VenueEventDrain {
    fn drain_queued_venue_events(&mut self) -> Result<VenueEventDrainReport, VenueEventDrainError>;
}

pub trait PreparedEpochRevaluator {
    fn revalue_under_prepared_epoch(
        &mut self,
        input: PreparedEpochRevaluationInput<'_>,
    ) -> Result<PostCutoverAdmissionState, PolicyEpochRevaluationError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyEpochPrepareError {
    InvalidBundle,
    Classification(RiskClassificationError),
    EnvelopeViolation(SafetyPolicyEnvelopeViolation),
    AttestationVerificationUnavailable,
    VenueEventDrain(VenueEventDrainError),
    RevaluationFailed(PolicyEpochRevaluationError),
    StateMutation(RiskStateMutationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyEpochActivationError {
    StateMutation(RiskStateMutationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyEpochRevaluationError {
    PartialFailure {
        revalued_item_count: usize,
        failed_item_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SafetyPolicyEnvelopeViolation {
    InvalidEnvelope,
    ScopeMismatch,
    ActivationHorizonExceeded,
    DescriptorCountOutOfRange {
        descriptor_count: usize,
        max_descriptor_count: usize,
    },
    TerminalStateCountOutOfRange {
        instrument_id: String,
        terminal_state_count: usize,
        max_terminal_state_count: usize,
    },
    TerminalCashFlowOutOfRange {
        instrument_id: String,
        terminal_state_index: usize,
        terminal_cash_flow: Decimal,
        min_terminal_cash_flow: Decimal,
        max_terminal_cash_flow: Decimal,
    },
    ModelVersionNotPermitted,
    FallbackModelVersionNotPermitted,
    ClassifierVersionNotPermitted,
    FeeModelVersionNotPermitted,
    SizingPolicyVersionNotPermitted {
        sizing_policy_version: String,
    },
    RequiredApprovalMissing {
        approval_id: String,
    },
    ApprovalDigestMismatch,
    DescriptorPolicyEpochMismatch {
        instrument_id: String,
        descriptor_policy_epoch_id: String,
        bundle_epoch_id: String,
    },
}

impl EpochManager {
    pub fn new(owner: RiskStateOwner) -> Self {
        Self { owner }
    }

    pub fn prepare_policy_epoch(
        &self,
        prepared_epoch: PreparedPolicyEpoch,
        envelope: SafetyPolicyEnvelope,
        requested_at_unix_nanos: u64,
        venue_event_drain: &mut impl VenueEventDrain,
        revaluator: &mut impl PreparedEpochRevaluator,
    ) -> Result<PreparedEpochCutover, PolicyEpochPrepareError> {
        validate_prepared_epoch(&prepared_epoch)?;
        validate_safety_envelope(&prepared_epoch, &envelope, requested_at_unix_nanos)?;

        let drain_report = venue_event_drain
            .drain_queued_venue_events()
            .map_err(PolicyEpochPrepareError::VenueEventDrain)?;
        let current_policy_state = self
            .owner
            .policy_epoch_snapshot()
            .map_err(PolicyEpochPrepareError::StateMutation)?;
        let revaluation_input = PreparedEpochRevaluationInput {
            prepared_epoch: &prepared_epoch,
            envelope: &envelope,
            drain_report,
            current_policy_state,
        };
        let post_cutover_admission_state =
            match revaluator.revalue_under_prepared_epoch(revaluation_input) {
                Ok(state) => state,
                Err(error) => {
                    self.owner
                        .commit_policy_epoch_no_new_risk_alert(
                            prepared_epoch.epoch_id.clone(),
                            PolicyEpochAlertReason::PartialRevaluationFailure,
                        )
                        .map_err(PolicyEpochPrepareError::StateMutation)?;
                    return Err(PolicyEpochPrepareError::RevaluationFailed(error));
                }
            };

        Ok(PreparedEpochCutover {
            prepared_epoch,
            envelope,
            drain_report,
            post_cutover_admission_state,
        })
    }

    pub fn activate_prepared_epoch(
        &self,
        prepared: PreparedEpochCutover,
    ) -> Result<PolicyEpochActivation, PolicyEpochActivationError> {
        let prior_epoch_id = self
            .owner
            .policy_epoch_snapshot()
            .map_err(PolicyEpochActivationError::StateMutation)?
            .active_epoch
            .map(|epoch| epoch.epoch_id);
        let risk_increasing_admission_enabled = prepared
            .post_cutover_admission_state
            .current_exposure_compliant;
        let safety_action_enabled = true;
        let snapshot = self
            .owner
            .commit_policy_epoch_cutover(
                prepared.prepared_epoch.clone(),
                risk_increasing_admission_enabled,
                safety_action_enabled,
            )
            .map_err(PolicyEpochActivationError::StateMutation)?;

        Ok(PolicyEpochActivation {
            prior_epoch_id,
            active_epoch_id: prepared.prepared_epoch.epoch_id,
            risk_state_version: snapshot.risk_state_version,
            risk_increasing_admission_enabled,
            safety_action_enabled,
        })
    }
}

fn validate_prepared_epoch(
    prepared_epoch: &PreparedPolicyEpoch,
) -> Result<(), PolicyEpochPrepareError> {
    if !is_clean_runtime_value(&prepared_epoch.epoch_id)
        || !is_clean_runtime_value(&prepared_epoch.environment)
        || !is_clean_runtime_value(&prepared_epoch.policy_digest)
        || !is_clean_runtime_value(&prepared_epoch.descriptor_map_digest)
        || !is_clean_runtime_value(&prepared_epoch.classifier_version)
        || !is_clean_runtime_value(&prepared_epoch.model_version)
        || !is_clean_runtime_value(&prepared_epoch.fallback_model_version)
        || !is_clean_runtime_value(&prepared_epoch.fee_model_version)
        || !is_clean_runtime_value(&prepared_epoch.approval_digest)
        || prepared_epoch.descriptor_map.is_empty()
        || prepared_epoch.sizing_policy_versions.is_empty()
        || prepared_epoch.approvals.is_empty()
    {
        return Err(PolicyEpochPrepareError::InvalidBundle);
    }

    for sizing_policy_version in &prepared_epoch.sizing_policy_versions {
        if !is_clean_runtime_value(sizing_policy_version) {
            return Err(PolicyEpochPrepareError::InvalidBundle);
        }
    }
    for approval in &prepared_epoch.approvals {
        validate_approval(approval)?;
    }
    for attestation in &prepared_epoch.declared_attestations {
        match attestation {
            PreparedEpochAttestation::BandCoverageAttestation { attestation_digest } => {
                if !is_clean_runtime_value(attestation_digest) {
                    return Err(PolicyEpochPrepareError::InvalidBundle);
                }
                // S6b handoff: replace this fail-closed stub with canonical-artifact
                // verification and digest binding for BandCoverageAttestation.
                return Err(PolicyEpochPrepareError::AttestationVerificationUnavailable);
            }
        }
    }

    for (instrument_id, descriptor) in &prepared_epoch.descriptor_map {
        validate_prepared_descriptor(instrument_id, descriptor)?;
        RiskClassifier::classify(
            &descriptor.descriptor_attributes,
            &prepared_epoch.classification_policy,
            &[],
        )
        .map_err(PolicyEpochPrepareError::Classification)?;
    }
    Ok(())
}

fn validate_safety_envelope(
    prepared_epoch: &PreparedPolicyEpoch,
    envelope: &SafetyPolicyEnvelope,
    requested_at_unix_nanos: u64,
) -> Result<(), PolicyEpochPrepareError> {
    if !is_clean_runtime_value(&envelope.envelope_id)
        || !is_clean_runtime_value(&envelope.envelope_version)
        || !is_clean_runtime_value(&envelope.environment)
        || !is_clean_runtime_value(&envelope.required_approval_digest)
        || envelope.ranges.max_descriptor_count == 0
        || envelope.ranges.max_terminal_states_per_descriptor == 0
        || envelope.ranges.max_sizing_policy_versions == 0
        || envelope.ranges.min_terminal_cash_flow > envelope.ranges.max_terminal_cash_flow
        || envelope.permitted_model_versions.is_empty()
        || envelope.permitted_fallback_model_versions.is_empty()
        || envelope.permitted_classifier_versions.is_empty()
        || envelope.permitted_fee_model_versions.is_empty()
        || envelope.permitted_sizing_policy_versions.is_empty()
        || envelope.required_approval_ids.is_empty()
        || envelope
            .permitted_model_versions
            .iter()
            .chain(envelope.permitted_fallback_model_versions.iter())
            .chain(envelope.permitted_classifier_versions.iter())
            .chain(envelope.permitted_fee_model_versions.iter())
            .chain(envelope.permitted_sizing_policy_versions.iter())
            .chain(envelope.required_approval_ids.iter())
            .any(|value| !is_clean_runtime_value(value))
    {
        return Err(PolicyEpochPrepareError::EnvelopeViolation(
            SafetyPolicyEnvelopeViolation::InvalidEnvelope,
        ));
    }
    if prepared_epoch.environment.as_str() != envelope.environment.as_str()
        || prepared_epoch.pool_id.as_str() != envelope.pool_id.as_str()
    {
        return Err(PolicyEpochPrepareError::EnvelopeViolation(
            SafetyPolicyEnvelopeViolation::ScopeMismatch,
        ));
    }
    if prepared_epoch.activation_not_after_unix_nanos
        > requested_at_unix_nanos.saturating_add(envelope.ranges.max_activation_horizon_unix_nanos)
    {
        return Err(PolicyEpochPrepareError::EnvelopeViolation(
            SafetyPolicyEnvelopeViolation::ActivationHorizonExceeded,
        ));
    }
    if prepared_epoch.descriptor_map.len() > envelope.ranges.max_descriptor_count {
        return Err(PolicyEpochPrepareError::EnvelopeViolation(
            SafetyPolicyEnvelopeViolation::DescriptorCountOutOfRange {
                descriptor_count: prepared_epoch.descriptor_map.len(),
                max_descriptor_count: envelope.ranges.max_descriptor_count,
            },
        ));
    }
    if prepared_epoch.sizing_policy_versions.len() > envelope.ranges.max_sizing_policy_versions {
        return Err(PolicyEpochPrepareError::EnvelopeViolation(
            SafetyPolicyEnvelopeViolation::InvalidEnvelope,
        ));
    }
    if !envelope
        .permitted_model_versions
        .contains(&prepared_epoch.model_version)
    {
        return Err(PolicyEpochPrepareError::EnvelopeViolation(
            SafetyPolicyEnvelopeViolation::ModelVersionNotPermitted,
        ));
    }
    if !envelope
        .permitted_fallback_model_versions
        .contains(&prepared_epoch.fallback_model_version)
    {
        return Err(PolicyEpochPrepareError::EnvelopeViolation(
            SafetyPolicyEnvelopeViolation::FallbackModelVersionNotPermitted,
        ));
    }
    if !envelope
        .permitted_classifier_versions
        .contains(&prepared_epoch.classifier_version)
    {
        return Err(PolicyEpochPrepareError::EnvelopeViolation(
            SafetyPolicyEnvelopeViolation::ClassifierVersionNotPermitted,
        ));
    }
    if !envelope
        .permitted_fee_model_versions
        .contains(&prepared_epoch.fee_model_version)
    {
        return Err(PolicyEpochPrepareError::EnvelopeViolation(
            SafetyPolicyEnvelopeViolation::FeeModelVersionNotPermitted,
        ));
    }
    for sizing_policy_version in &prepared_epoch.sizing_policy_versions {
        if !envelope
            .permitted_sizing_policy_versions
            .contains(sizing_policy_version)
        {
            return Err(PolicyEpochPrepareError::EnvelopeViolation(
                SafetyPolicyEnvelopeViolation::SizingPolicyVersionNotPermitted {
                    sizing_policy_version: sizing_policy_version.clone(),
                },
            ));
        }
    }
    validate_required_approvals(prepared_epoch, envelope)?;
    validate_descriptor_ranges(prepared_epoch, envelope)?;
    validate_envelope_invariants(prepared_epoch, envelope)?;
    Ok(())
}

fn validate_approval(approval: &PolicyApproval) -> Result<(), PolicyEpochPrepareError> {
    if !is_clean_runtime_value(&approval.approval_id)
        || !is_clean_runtime_value(&approval.approver_id)
    {
        return Err(PolicyEpochPrepareError::InvalidBundle);
    }
    Ok(())
}

fn validate_prepared_descriptor(
    descriptor_map_key: &str,
    descriptor: &PreparedEpochDescriptor,
) -> Result<(), PolicyEpochPrepareError> {
    if !is_clean_runtime_value(descriptor_map_key)
        || descriptor_map_key != descriptor.active_descriptor.instrument_id.as_str()
    {
        return Err(PolicyEpochPrepareError::InvalidBundle);
    }
    validate_active_descriptor(&descriptor.active_descriptor)
}

fn validate_active_descriptor(
    descriptor: &ActiveDescriptorView,
) -> Result<(), PolicyEpochPrepareError> {
    if !is_clean_runtime_value(&descriptor.instrument_id)
        || !is_clean_runtime_value(&descriptor.descriptor_version)
        || !is_clean_runtime_value(&descriptor.policy_epoch_id)
        || descriptor.terminal_state_ids.is_empty()
        || descriptor.terminal_state_ids.len() != descriptor.terminal_cash_flows.len()
        || descriptor
            .terminal_state_ids
            .iter()
            .any(|state_id| !is_clean_runtime_value(state_id))
    {
        return Err(PolicyEpochPrepareError::InvalidBundle);
    }
    Ok(())
}

fn validate_required_approvals(
    prepared_epoch: &PreparedPolicyEpoch,
    envelope: &SafetyPolicyEnvelope,
) -> Result<(), PolicyEpochPrepareError> {
    if prepared_epoch.approval_digest.as_str() != envelope.required_approval_digest.as_str() {
        return Err(PolicyEpochPrepareError::EnvelopeViolation(
            SafetyPolicyEnvelopeViolation::ApprovalDigestMismatch,
        ));
    }
    for required_approval_id in &envelope.required_approval_ids {
        if !prepared_epoch
            .approvals
            .iter()
            .any(|approval| &approval.approval_id == required_approval_id)
        {
            return Err(PolicyEpochPrepareError::EnvelopeViolation(
                SafetyPolicyEnvelopeViolation::RequiredApprovalMissing {
                    approval_id: required_approval_id.clone(),
                },
            ));
        }
    }
    Ok(())
}

fn validate_descriptor_ranges(
    prepared_epoch: &PreparedPolicyEpoch,
    envelope: &SafetyPolicyEnvelope,
) -> Result<(), PolicyEpochPrepareError> {
    for descriptor in prepared_epoch.descriptor_map.values() {
        let terminal_state_count = descriptor.active_descriptor.terminal_state_ids.len();
        if terminal_state_count > envelope.ranges.max_terminal_states_per_descriptor {
            return Err(PolicyEpochPrepareError::EnvelopeViolation(
                SafetyPolicyEnvelopeViolation::TerminalStateCountOutOfRange {
                    instrument_id: descriptor.active_descriptor.instrument_id.clone(),
                    terminal_state_count,
                    max_terminal_state_count: envelope.ranges.max_terminal_states_per_descriptor,
                },
            ));
        }
        for (terminal_state_index, terminal_cash_flow) in descriptor
            .active_descriptor
            .terminal_cash_flows
            .iter()
            .copied()
            .enumerate()
        {
            if terminal_cash_flow < envelope.ranges.min_terminal_cash_flow
                || terminal_cash_flow > envelope.ranges.max_terminal_cash_flow
            {
                return Err(PolicyEpochPrepareError::EnvelopeViolation(
                    SafetyPolicyEnvelopeViolation::TerminalCashFlowOutOfRange {
                        instrument_id: descriptor.active_descriptor.instrument_id.clone(),
                        terminal_state_index,
                        terminal_cash_flow,
                        min_terminal_cash_flow: envelope.ranges.min_terminal_cash_flow,
                        max_terminal_cash_flow: envelope.ranges.max_terminal_cash_flow,
                    },
                ));
            }
        }
    }
    Ok(())
}

fn validate_envelope_invariants(
    prepared_epoch: &PreparedPolicyEpoch,
    envelope: &SafetyPolicyEnvelope,
) -> Result<(), PolicyEpochPrepareError> {
    if envelope
        .invariants
        .contains(&SafetyEnvelopeInvariant::DescriptorPolicyEpochMatchesBundle)
    {
        for descriptor in prepared_epoch.descriptor_map.values() {
            if descriptor.active_descriptor.policy_epoch_id.as_str()
                != prepared_epoch.epoch_id.as_str()
            {
                return Err(PolicyEpochPrepareError::EnvelopeViolation(
                    SafetyPolicyEnvelopeViolation::DescriptorPolicyEpochMismatch {
                        instrument_id: descriptor.active_descriptor.instrument_id.clone(),
                        descriptor_policy_epoch_id: descriptor
                            .active_descriptor
                            .policy_epoch_id
                            .clone(),
                        bundle_epoch_id: prepared_epoch.epoch_id.clone(),
                    },
                ));
            }
        }
    }
    Ok(())
}

fn is_clean_runtime_value(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}
