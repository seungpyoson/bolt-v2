use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

use rust_decimal::Decimal;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    bolt_v3_numeric::is_sha256_hex_digest,
    bolt_v3_risk_reservation_substrate::{
        contracts::{ActiveDescriptorView, AdmissionCandidate},
        risk_classifier::RiskDescriptorCanonicalAttributes,
    },
};

#[derive(Debug, Clone)]
pub struct InstrumentRiskRegistry {
    inner: Arc<Mutex<InstrumentRiskRegistryInner>>,
}

#[derive(Debug, Clone)]
struct InstrumentRiskRegistryInner {
    descriptors: BTreeMap<DescriptorKey, RegisteredDescriptor>,
    active_versions: BTreeMap<ActiveDescriptorKey, String>,
    halted_unknown_states: BTreeMap<ActiveDescriptorKey, DescriptorTerminalStateEnvelope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstrumentRiskDescriptor {
    pub instrument_id: String,
    pub descriptor_version: String,
    pub policy_epoch_id: String,
    pub terminal_state_ids: Vec<String>,
    pub terminal_cash_flows: Vec<Decimal>,
    pub unknown_state_envelope: DescriptorTerminalStateEnvelope,
    pub canonical_attributes: RiskDescriptorCanonicalAttributes,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorTerminalStateEnvelope {
    pub terminal_state_id: String,
    pub terminal_cash_flow: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorCoverageAttestation {
    pub descriptor_digest: String,
    pub producer_identity: String,
    pub certifier_identity: String,
    pub decision: DescriptorCertificationDecision,
    pub evidence: DescriptorCertificationEvidence,
    pub valid_from_unix_nanos: u64,
    pub valid_until_unix_nanos: u64,
    pub revoked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorCertificationDecision {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescriptorCertificationEvidence {
    pub evidence_digest: String,
    pub terminal_state_count: usize,
    pub classification_attribute_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertifiedActiveDescriptor {
    pub active_descriptor: ActiveDescriptorView,
    pub descriptor_attributes: RiskDescriptorCanonicalAttributes,
    pub descriptor_digest: String,
    pub certifier_identity: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalStateObservation {
    Known {
        terminal_state_id: String,
        terminal_cash_flow: Decimal,
    },
    Unknown(DescriptorTerminalStateEnvelope),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorActivationStatus {
    InitialActivation,
    AlreadyActive,
    SupersededVersionRequiresRevaluation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DescriptorRegistryError {
    InvalidDescriptor,
    UncertifiedDescriptor,
    InvalidAttestation,
    AttestationDigestMismatch,
    ProducerCertifierIdentityCollision,
    ImmutableVersionMutationRejected,
    DescriptorVersionAlreadyRegistered,
    DescriptorVersionUnknown,
    NoActiveDescriptor,
    CanonicalDigestUnavailable,
    AmbiguousRegistryState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DescriptorRegistryAdmissionError {
    NoActiveDescriptor,
    DescriptorVersionMismatch {
        active_descriptor_version: String,
        candidate_descriptor_version: String,
    },
    ActiveDescriptorViewMismatch,
    CertifierMatchesAdmissionIdentity,
    AdmissionHaltedByUnknownState {
        envelope: DescriptorTerminalStateEnvelope,
    },
    RegistryUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ActiveDescriptorKey {
    instrument_id: String,
    policy_epoch_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct DescriptorKey {
    active: ActiveDescriptorKey,
    descriptor_version: String,
}

#[derive(Debug, Clone)]
struct RegisteredDescriptor {
    descriptor: InstrumentRiskDescriptor,
    attestation: DescriptorCoverageAttestation,
    descriptor_digest: String,
}

impl InstrumentRiskRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(InstrumentRiskRegistryInner {
                descriptors: BTreeMap::new(),
                active_versions: BTreeMap::new(),
                halted_unknown_states: BTreeMap::new(),
            })),
        }
    }

    pub fn register_descriptor_version(
        &mut self,
        descriptor: InstrumentRiskDescriptor,
        attestation: Option<DescriptorCoverageAttestation>,
        now_unix_nanos: u64,
    ) -> Result<(), DescriptorRegistryError> {
        descriptor.validate()?;
        let Some(attestation) = attestation else {
            return Err(DescriptorRegistryError::UncertifiedDescriptor);
        };
        let descriptor_digest = descriptor.canonical_digest()?;
        validate_attestation(
            &descriptor,
            &attestation,
            &descriptor_digest,
            now_unix_nanos,
        )?;

        let key = DescriptorKey::from_descriptor(&descriptor);
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| DescriptorRegistryError::AmbiguousRegistryState)?;
        if let Some(existing) = inner.descriptors.get(&key) {
            if existing.descriptor_digest != descriptor_digest {
                return Err(DescriptorRegistryError::ImmutableVersionMutationRejected);
            }
            return Err(DescriptorRegistryError::DescriptorVersionAlreadyRegistered);
        }

        inner.descriptors.insert(
            key,
            RegisteredDescriptor {
                descriptor,
                attestation,
                descriptor_digest,
            },
        );
        Ok(())
    }

    pub fn activate_descriptor_version(
        &mut self,
        instrument_id: &str,
        policy_epoch_id: &str,
        descriptor_version: &str,
    ) -> Result<DescriptorActivationStatus, DescriptorRegistryError> {
        let active = ActiveDescriptorKey::new(instrument_id, policy_epoch_id)?;
        if !is_clean_runtime_value(descriptor_version) {
            return Err(DescriptorRegistryError::InvalidDescriptor);
        }
        let descriptor_key = DescriptorKey {
            active: active.clone(),
            descriptor_version: descriptor_version.to_string(),
        };
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| DescriptorRegistryError::AmbiguousRegistryState)?;
        if !inner.descriptors.contains_key(&descriptor_key) {
            return Err(DescriptorRegistryError::DescriptorVersionUnknown);
        }
        match inner
            .active_versions
            .insert(active, descriptor_version.to_string())
        {
            None => Ok(DescriptorActivationStatus::InitialActivation),
            Some(previous) if previous == descriptor_version => {
                Ok(DescriptorActivationStatus::AlreadyActive)
            }
            Some(_) => Ok(DescriptorActivationStatus::SupersededVersionRequiresRevaluation),
        }
    }

    pub fn resolve_active_descriptor(
        &self,
        instrument_id: &str,
        policy_epoch_id: &str,
    ) -> Result<CertifiedActiveDescriptor, DescriptorRegistryError> {
        let active = ActiveDescriptorKey::new(instrument_id, policy_epoch_id)?;
        let inner = self
            .inner
            .lock()
            .map_err(|_| DescriptorRegistryError::AmbiguousRegistryState)?;
        let descriptor_version = inner
            .active_versions
            .get(&active)
            .ok_or(DescriptorRegistryError::NoActiveDescriptor)?;
        let descriptor_key = DescriptorKey {
            active,
            descriptor_version: descriptor_version.clone(),
        };
        let registered = inner
            .descriptors
            .get(&descriptor_key)
            .ok_or(DescriptorRegistryError::DescriptorVersionUnknown)?;
        Ok(registered.certified_active_descriptor())
    }

    pub fn validate_admission_binding(
        &self,
        view_descriptor: &ActiveDescriptorView,
        candidate: &AdmissionCandidate,
        admission_identity: &str,
    ) -> Result<(), DescriptorRegistryAdmissionError> {
        let active = self
            .resolve_active_descriptor(&candidate.instrument_id, &candidate.policy_epoch_id)
            .map_err(map_registry_admission_error)?;
        if candidate.expected_descriptor_version != active.active_descriptor.descriptor_version {
            return Err(
                DescriptorRegistryAdmissionError::DescriptorVersionMismatch {
                    active_descriptor_version: active.active_descriptor.descriptor_version,
                    candidate_descriptor_version: candidate.expected_descriptor_version.clone(),
                },
            );
        }
        if view_descriptor != &active.active_descriptor {
            return Err(DescriptorRegistryAdmissionError::ActiveDescriptorViewMismatch);
        }
        if active.certifier_identity == admission_identity {
            return Err(DescriptorRegistryAdmissionError::CertifierMatchesAdmissionIdentity);
        }
        let active_key = ActiveDescriptorKey {
            instrument_id: candidate.instrument_id.clone(),
            policy_epoch_id: candidate.policy_epoch_id.clone(),
        };
        let inner = self
            .inner
            .lock()
            .map_err(|_| DescriptorRegistryAdmissionError::RegistryUnavailable)?;
        if let Some(envelope) = inner.halted_unknown_states.get(&active_key) {
            return Err(
                DescriptorRegistryAdmissionError::AdmissionHaltedByUnknownState {
                    envelope: envelope.clone(),
                },
            );
        }
        Ok(())
    }

    pub fn observe_terminal_state(
        &mut self,
        instrument_id: &str,
        policy_epoch_id: &str,
        terminal_state_id: &str,
    ) -> Result<TerminalStateObservation, DescriptorRegistryError> {
        let active = ActiveDescriptorKey::new(instrument_id, policy_epoch_id)?;
        if !is_clean_runtime_value(terminal_state_id) {
            return Err(DescriptorRegistryError::InvalidDescriptor);
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| DescriptorRegistryError::AmbiguousRegistryState)?;
        let descriptor_version = inner
            .active_versions
            .get(&active)
            .ok_or(DescriptorRegistryError::NoActiveDescriptor)?
            .clone();
        let descriptor_key = DescriptorKey {
            active: active.clone(),
            descriptor_version,
        };
        let registered = inner
            .descriptors
            .get(&descriptor_key)
            .ok_or(DescriptorRegistryError::DescriptorVersionUnknown)?;
        if let Some((index, state_id)) = registered
            .descriptor
            .terminal_state_ids
            .iter()
            .enumerate()
            .find(|(_, state_id)| state_id.as_str() == terminal_state_id)
        {
            return Ok(TerminalStateObservation::Known {
                terminal_state_id: state_id.clone(),
                terminal_cash_flow: registered.descriptor.terminal_cash_flows[index],
            });
        }
        let envelope = registered.descriptor.unknown_state_envelope.clone();
        inner.halted_unknown_states.insert(active, envelope.clone());
        Ok(TerminalStateObservation::Unknown(envelope))
    }
}

impl Default for InstrumentRiskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl InstrumentRiskDescriptor {
    pub fn new(
        instrument_id: String,
        descriptor_version: String,
        policy_epoch_id: String,
        terminal_state_ids: Vec<String>,
        terminal_cash_flows: Vec<Decimal>,
        unknown_state_envelope: DescriptorTerminalStateEnvelope,
        canonical_attributes: RiskDescriptorCanonicalAttributes,
    ) -> Result<Self, DescriptorRegistryError> {
        let descriptor = Self {
            instrument_id,
            descriptor_version,
            policy_epoch_id,
            terminal_state_ids,
            terminal_cash_flows,
            unknown_state_envelope,
            canonical_attributes,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub fn canonical_digest(&self) -> Result<String, DescriptorRegistryError> {
        self.validate()?;
        let input = DescriptorDigestInput {
            instrument_id: &self.instrument_id,
            descriptor_version: &self.descriptor_version,
            policy_epoch_id: &self.policy_epoch_id,
            terminal_state_ids: &self.terminal_state_ids,
            terminal_cash_flows: self
                .terminal_cash_flows
                .iter()
                .map(|value| value.to_string())
                .collect(),
            unknown_state_envelope: DescriptorEnvelopeDigestInput {
                terminal_state_id: &self.unknown_state_envelope.terminal_state_id,
                terminal_cash_flow: self.unknown_state_envelope.terminal_cash_flow.to_string(),
            },
            canonical_attributes: self.canonical_attributes.attributes(),
        };
        let bytes = serde_json::to_vec(&input)
            .map_err(|_| DescriptorRegistryError::CanonicalDigestUnavailable)?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }

    fn validate(&self) -> Result<(), DescriptorRegistryError> {
        if !is_clean_runtime_value(&self.instrument_id)
            || !is_clean_runtime_value(&self.descriptor_version)
            || !is_clean_runtime_value(&self.policy_epoch_id)
            || !is_clean_runtime_value(&self.unknown_state_envelope.terminal_state_id)
            || self.terminal_state_ids.is_empty()
            || self.terminal_state_ids.len() != self.terminal_cash_flows.len()
            || self
                .terminal_state_ids
                .iter()
                .any(|terminal_state_id| !is_clean_runtime_value(terminal_state_id))
        {
            return Err(DescriptorRegistryError::InvalidDescriptor);
        }
        let unique_states = self
            .terminal_state_ids
            .iter()
            .collect::<BTreeSet<&String>>();
        if unique_states.len() != self.terminal_state_ids.len() {
            return Err(DescriptorRegistryError::InvalidDescriptor);
        }
        Ok(())
    }
}

impl RegisteredDescriptor {
    fn certified_active_descriptor(&self) -> CertifiedActiveDescriptor {
        CertifiedActiveDescriptor {
            active_descriptor: ActiveDescriptorView {
                instrument_id: self.descriptor.instrument_id.clone(),
                descriptor_version: self.descriptor.descriptor_version.clone(),
                policy_epoch_id: self.descriptor.policy_epoch_id.clone(),
                terminal_state_ids: self.descriptor.terminal_state_ids.clone(),
                terminal_cash_flows: self.descriptor.terminal_cash_flows.clone(),
            },
            descriptor_attributes: self.descriptor.canonical_attributes.clone(),
            descriptor_digest: self.descriptor_digest.clone(),
            certifier_identity: self.attestation.certifier_identity.clone(),
        }
    }
}

impl ActiveDescriptorKey {
    fn new(instrument_id: &str, policy_epoch_id: &str) -> Result<Self, DescriptorRegistryError> {
        if !is_clean_runtime_value(instrument_id) || !is_clean_runtime_value(policy_epoch_id) {
            return Err(DescriptorRegistryError::InvalidDescriptor);
        }
        Ok(Self {
            instrument_id: instrument_id.to_string(),
            policy_epoch_id: policy_epoch_id.to_string(),
        })
    }
}

impl DescriptorKey {
    fn from_descriptor(descriptor: &InstrumentRiskDescriptor) -> Self {
        Self {
            active: ActiveDescriptorKey {
                instrument_id: descriptor.instrument_id.clone(),
                policy_epoch_id: descriptor.policy_epoch_id.clone(),
            },
            descriptor_version: descriptor.descriptor_version.clone(),
        }
    }
}

#[derive(Serialize)]
struct DescriptorDigestInput<'a> {
    instrument_id: &'a str,
    descriptor_version: &'a str,
    policy_epoch_id: &'a str,
    terminal_state_ids: &'a [String],
    terminal_cash_flows: Vec<String>,
    unknown_state_envelope: DescriptorEnvelopeDigestInput<'a>,
    canonical_attributes: &'a BTreeMap<String, String>,
}

#[derive(Serialize)]
struct DescriptorEnvelopeDigestInput<'a> {
    terminal_state_id: &'a str,
    terminal_cash_flow: String,
}

fn validate_attestation(
    descriptor: &InstrumentRiskDescriptor,
    attestation: &DescriptorCoverageAttestation,
    descriptor_digest: &str,
    now_unix_nanos: u64,
) -> Result<(), DescriptorRegistryError> {
    if attestation.decision != DescriptorCertificationDecision::Approved
        || attestation.revoked
        || attestation.valid_from_unix_nanos >= attestation.valid_until_unix_nanos
        || now_unix_nanos < attestation.valid_from_unix_nanos
        || now_unix_nanos >= attestation.valid_until_unix_nanos
        || !is_sha256_hex_digest(&attestation.descriptor_digest)
        || !is_sha256_hex_digest(&attestation.evidence.evidence_digest)
        || !is_clean_runtime_value(&attestation.producer_identity)
        || !is_clean_runtime_value(&attestation.certifier_identity)
        || attestation.evidence.terminal_state_count != descriptor.terminal_state_ids.len()
        || attestation.evidence.classification_attribute_count
            != descriptor.canonical_attributes.attributes().len()
    {
        return Err(DescriptorRegistryError::InvalidAttestation);
    }
    if attestation.producer_identity == attestation.certifier_identity {
        return Err(DescriptorRegistryError::ProducerCertifierIdentityCollision);
    }
    if attestation.descriptor_digest != descriptor_digest {
        return Err(DescriptorRegistryError::AttestationDigestMismatch);
    }
    Ok(())
}

fn map_registry_admission_error(
    error: DescriptorRegistryError,
) -> DescriptorRegistryAdmissionError {
    match error {
        DescriptorRegistryError::NoActiveDescriptor => {
            DescriptorRegistryAdmissionError::NoActiveDescriptor
        }
        _ => DescriptorRegistryAdmissionError::RegistryUnavailable,
    }
}

fn is_clean_runtime_value(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}
