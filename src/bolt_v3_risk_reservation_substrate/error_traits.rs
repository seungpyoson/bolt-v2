use std::fmt;

use crate::bolt_v3_risk_reservation_substrate::{
    admission_service::{CertifiedAdmissionReserveError, SafetyActionAdmissionError},
    contracts::{
        BandCoverageAttestationDigestError, ContractIdentityError, FencingTokenError,
        LeaseAuthorityConfigError, RiskReservationOfferedLoadEnvelopeError,
        RiskReservationWorkBoundsError, RiskStateVersionError,
    },
    epoch_manager::{
        BandCoverageAttestationError, PolicyEpochActivationError, PolicyEpochPrepareError,
        PolicyEpochRevaluationError, VenueEventDrainError,
    },
    instrument_risk_registry::{DescriptorRegistryAdmissionError, DescriptorRegistryError},
    lifecycle_reconciler::LifecycleReconciliationError,
    reservation_ledger::RiskReservationError,
    risk_classifier::RiskClassificationError,
    risk_kernel::RiskKernelError,
    risk_view_publisher::{RiskPreviewError, RiskViewPublishError},
    state_owner::{RiskStateMutationError, RiskSubmissionMutationError},
    submission_authority::SubmissionAuthorityError,
};

macro_rules! impl_error_without_source {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl std::error::Error for $ty {}
        )+
    };
}

impl fmt::Display for CertifiedAdmissionReserveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registry(error) => write!(f, "{error}"),
            Self::Reserve(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CertifiedAdmissionReserveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Registry(error) => Some(error),
            Self::Reserve(error) => Some(error),
        }
    }
}

impl fmt::Display for SafetyActionAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAction => write!(f, "invalid safety action"),
            Self::InvalidProofDomain => write!(f, "invalid safety action proof domain"),
            Self::SafetyStateVersionMismatch { .. } => {
                write!(f, "safety state version mismatch")
            }
            Self::ProofDomainExceeded { .. } => {
                write!(f, "safety action proof domain exceeded")
            }
            Self::UnknownSafetyActionTarget => write!(f, "unknown safety action target"),
            Self::Kernel(error) => write!(f, "{error}"),
            Self::AfterExposureNotReduction { .. } => {
                write!(f, "safety action does not reduce exposure")
            }
            Self::RiskIncreased { .. } => write!(f, "safety action increases risk"),
            Self::StateMutation(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SafetyActionAdmissionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Kernel(error) => Some(error),
            Self::StateMutation(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for RiskReservationOfferedLoadEnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroMaxSupportedInFlightRiskIncreasingAdmissions => {
                write!(f, "offered load envelope limit is zero")
            }
        }
    }
}

impl fmt::Display for RiskReservationWorkBoundsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroCurrentPositionCount => write!(f, "current position count is zero"),
            Self::ZeroBucketsPerExposure => write!(f, "buckets per exposure is zero"),
            Self::ZeroTerminalCashFlowCountPerExposure => {
                write!(f, "terminal cash flow count per exposure is zero")
            }
        }
    }
}

impl fmt::Display for BandCoverageAttestationDigestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CanonicalDigestUnavailable => {
                write!(f, "band coverage attestation canonical digest unavailable")
            }
        }
    }
}

impl fmt::Display for ContractIdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPoolId => write!(f, "invalid pool id"),
            Self::InvalidOwnerId => write!(f, "invalid owner id"),
        }
    }
}

impl fmt::Display for RiskStateVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Overflow => write!(f, "risk state version overflow"),
        }
    }
}

impl fmt::Display for FencingTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => write!(f, "fencing token is zero"),
            Self::Overflow => write!(f, "fencing token overflow"),
        }
    }
}

impl_error_without_source!(
    RiskReservationOfferedLoadEnvelopeError,
    RiskReservationWorkBoundsError,
    BandCoverageAttestationDigestError,
    ContractIdentityError,
    RiskStateVersionError,
    FencingTokenError,
    LeaseAuthorityConfigError,
);

impl fmt::Display for VenueEventDrainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DrainFailed => write!(f, "venue event drain failed"),
        }
    }
}

impl fmt::Display for PolicyEpochPrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBundle => write!(f, "invalid policy epoch bundle"),
            Self::Classification(error) => write!(f, "{error}"),
            Self::EnvelopeViolation(_) => write!(f, "safety policy envelope violation"),
            Self::BandCoverageAttestation(error) => write!(f, "{error}"),
            Self::VenueEventDrain(error) => write!(f, "{error}"),
            Self::RevaluationFailed(error) => write!(f, "{error}"),
            Self::StateMutation(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PolicyEpochPrepareError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Classification(error) => Some(error),
            Self::BandCoverageAttestation(error) => Some(error),
            Self::VenueEventDrain(error) => Some(error),
            Self::RevaluationFailed(error) => Some(error),
            Self::StateMutation(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for BandCoverageAttestationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingArtifact => write!(f, "band coverage attestation artifact missing"),
            Self::InvalidDigest => write!(f, "invalid band coverage attestation digest"),
            Self::DecisionNotApproved => {
                write!(f, "band coverage attestation decision not approved")
            }
            Self::Revoked => write!(f, "band coverage attestation revoked"),
            Self::InvalidValidityWindow => {
                write!(f, "invalid band coverage attestation validity window")
            }
            Self::Expired => write!(f, "band coverage attestation expired"),
            Self::InvalidIdentity => write!(f, "invalid band coverage attestation identity"),
            Self::ProducerCertifierIdentityCollision => {
                write!(
                    f,
                    "invalid band coverage attestation producer certifier identity collision"
                )
            }
            Self::MissingEvidenceField => {
                write!(f, "band coverage attestation evidence field missing")
            }
            Self::EligibilityRejected => {
                write!(f, "band coverage attestation eligibility rejected")
            }
            Self::DigestMismatch => write!(f, "band coverage attestation digest mismatch"),
            Self::CanonicalDigestUnavailable => {
                write!(f, "band coverage attestation canonical digest unavailable")
            }
        }
    }
}

impl fmt::Display for PolicyEpochActivationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StateMutation(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for PolicyEpochActivationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::StateMutation(error) => Some(error),
        }
    }
}

impl fmt::Display for PolicyEpochRevaluationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PartialFailure { .. } => write!(f, "policy epoch revaluation partial failure"),
        }
    }
}

impl_error_without_source!(
    VenueEventDrainError,
    BandCoverageAttestationError,
    PolicyEpochRevaluationError,
);

impl fmt::Display for DescriptorRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidDescriptor => write!(f, "invalid descriptor"),
            Self::UncertifiedDescriptor => write!(f, "uncertified descriptor"),
            Self::InvalidAttestation => write!(f, "invalid attestation"),
            Self::AttestationDigestMismatch => write!(f, "attestation digest mismatch"),
            Self::ProducerCertifierIdentityCollision => {
                write!(f, "producer certifier identity collision")
            }
            Self::ImmutableVersionMutationRejected => {
                write!(f, "immutable version mutation rejected")
            }
            Self::DescriptorVersionAlreadyRegistered => {
                write!(f, "descriptor version already registered")
            }
            Self::DescriptorVersionUnknown => write!(f, "descriptor version unknown"),
            Self::NoActiveDescriptor => write!(f, "no active descriptor"),
            Self::CanonicalDigestUnavailable => write!(f, "canonical digest unavailable"),
            Self::AmbiguousRegistryState => write!(f, "ambiguous registry state"),
        }
    }
}

impl fmt::Display for DescriptorRegistryAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoActiveDescriptor => write!(f, "no active descriptor"),
            Self::DescriptorVersionMismatch { .. } => write!(f, "descriptor version mismatch"),
            Self::ActiveDescriptorViewMismatch => write!(f, "active descriptor view mismatch"),
            Self::CertifierMatchesAdmissionIdentity => {
                write!(f, "certifier matches admission identity")
            }
            Self::AdmissionHaltedByUnknownState { .. } => {
                write!(f, "admission halted by unknown state")
            }
            Self::RegistryUnavailable => write!(f, "registry unavailable"),
        }
    }
}

impl_error_without_source!(DescriptorRegistryError, DescriptorRegistryAdmissionError);

impl fmt::Display for LifecycleReconciliationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(error) => write!(f, "{error}"),
            Self::ReservationRead(error) => write!(f, "{error}"),
            Self::Submit(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LifecycleReconciliationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::State(error) => Some(error),
            Self::ReservationRead(error) => Some(error),
            Self::Submit(error) => Some(error),
        }
    }
}

impl fmt::Display for RiskReservationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleRiskStateVersion { .. } => write!(f, "stale risk state version"),
            Self::SafetyStateVersionMismatch { .. } => {
                write!(f, "safety state version mismatch")
            }
            Self::PoolMismatch => write!(f, "pool mismatch"),
            Self::CandidateExpired => write!(f, "candidate expired"),
            Self::PermitVersionMismatch => write!(f, "permit version mismatch"),
            Self::PermitAlreadyConsumed => write!(f, "permit already consumed"),
            Self::IdempotencyConflict => write!(f, "idempotency conflict"),
            Self::NoActivePolicyEpoch => write!(f, "no active policy epoch"),
            Self::RiskIncreasingAdmissionDisabled => {
                write!(f, "risk increasing admission disabled")
            }
            Self::ActivePolicyEpochMismatch { .. } => write!(f, "active policy epoch mismatch"),
            Self::AdmissionShed { .. } => write!(f, "admission shed"),
            Self::WorkBoundExceeded { .. } => write!(f, "risk reservation work bound exceeded"),
            Self::InvalidCandidate => write!(f, "invalid candidate"),
            Self::Kernel(error) => write!(f, "{error}"),
            Self::Rejected(_) => write!(f, "risk reservation rejected"),
            Self::StateMutation(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RiskReservationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Kernel(error) => Some(error),
            Self::StateMutation(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for RiskClassificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBucketClass => write!(f, "invalid bucket class"),
            Self::InvalidBucketValue => write!(f, "invalid bucket value"),
            Self::InvalidCanonicalAttribute => write!(f, "invalid canonical attribute"),
            Self::InvalidCanonicalAttributeValue => {
                write!(f, "invalid canonical attribute value")
            }
            Self::MissingBucketDimensions => write!(f, "missing bucket dimensions"),
            Self::DuplicateBucketDimension { .. } => write!(f, "duplicate bucket dimension"),
            Self::MissingCanonicalAttribute { .. } => write!(f, "missing canonical attribute"),
        }
    }
}

impl fmt::Display for RiskKernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRiskInput => write!(f, "invalid risk input"),
            Self::UnrecognizedEvaluationScope => write!(f, "unrecognized evaluation scope"),
        }
    }
}

impl_error_without_source!(RiskClassificationError, RiskKernelError);

impl fmt::Display for RiskViewPublishError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSizingView => write!(f, "invalid sizing view"),
            Self::InvalidActiveDescriptor => write!(f, "invalid active descriptor"),
            Self::Classification(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RiskViewPublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Classification(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for RiskPreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleRiskStateVersion => write!(f, "stale risk state version"),
            Self::PolicyEpochMismatch => write!(f, "policy epoch mismatch"),
            Self::InstrumentMismatch => write!(f, "instrument mismatch"),
            Self::DescriptorVersionMismatch => write!(f, "descriptor version mismatch"),
            Self::InvalidCandidate => write!(f, "invalid candidate"),
            Self::Kernel(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RiskPreviewError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Kernel(error) => Some(error),
            _ => None,
        }
    }
}

impl fmt::Display for RiskStateMutationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidOwner => write!(f, "invalid owner"),
            Self::InvalidMutation => write!(f, "invalid mutation"),
            Self::AmbiguousLeaseState => write!(f, "ambiguous lease state"),
            Self::StaleFencingToken => write!(f, "stale fencing token"),
            Self::StaleRiskStateVersion => write!(f, "stale risk state version"),
            Self::ReconciliationRequired => write!(f, "reconciliation required"),
            Self::SafetyActionDisabled => write!(f, "safety action disabled"),
            Self::VersionOverflow => write!(f, "version overflow"),
        }
    }
}

impl fmt::Display for RiskSubmissionMutationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(error) => write!(f, "{error}"),
            Self::RiskKernel(error) => write!(f, "{error}"),
            Self::UnknownReservation => write!(f, "unknown reservation"),
            Self::UnknownSubmissionIntent => write!(f, "unknown submission intent"),
            Self::AdmissionTokenMismatch => write!(f, "admission token mismatch"),
            Self::ReservationNotReserved => write!(f, "reservation not reserved"),
            Self::SubmissionIntentConflict => write!(f, "submission intent conflict"),
            Self::InvalidLifecycleTransition => write!(f, "invalid lifecycle transition"),
        }
    }
}

impl std::error::Error for RiskSubmissionMutationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::State(error) => Some(error),
            Self::RiskKernel(error) => Some(error),
            _ => None,
        }
    }
}

impl_error_without_source!(RiskStateMutationError);

impl fmt::Display for SubmissionAuthorityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::State(error) => write!(f, "{error}"),
            Self::LiveSubmitRejected => write!(f, "live submit rejected"),
        }
    }
}

impl std::error::Error for SubmissionAuthorityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::State(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    #[test]
    fn wrapping_error_sources_and_display_messages_are_standardized() {
        let wrapped =
            PolicyEpochPrepareError::Classification(RiskClassificationError::InvalidBucketClass);

        assert!(wrapped.source().is_some());
        assert_eq!(wrapped.to_string(), "invalid bucket class");
        assert_eq!(
            ContractIdentityError::InvalidPoolId.to_string(),
            "invalid pool id"
        );
        assert_eq!(
            LeaseAuthorityConfigError::InvalidDependencyName.to_string(),
            "lease authority dependency name is invalid"
        );
    }
}
