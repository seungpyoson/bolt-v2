use crate::{
    bolt_v3_evidence_values::number as evidence_number, bolt_v3_realized_volatility as source,
};

use super::{
    RealizedVolAggregation, RealizedVolBlockReason, RealizedVolPricingComponent,
    RealizedVolSampleKind, RealizedVolSourceClass, RealizedVolSourceRejectReason,
    RealizedVolSourceStatus, RealizedVolatilitySourceDiagnosticFact,
};

impl From<source::RealizedVolPricingComponent> for RealizedVolPricingComponent {
    fn from(value: source::RealizedVolPricingComponent) -> Self {
        match value {
            source::RealizedVolPricingComponent::Measured => Self::Measured,
            source::RealizedVolPricingComponent::NoiseRobust => Self::NoiseRobust,
            source::RealizedVolPricingComponent::Continuous => Self::Continuous,
            source::RealizedVolPricingComponent::Forecast => Self::Forecast,
        }
    }
}

impl From<source::RealizedVolAggregation> for RealizedVolAggregation {
    fn from(value: source::RealizedVolAggregation) -> Self {
        match value {
            source::RealizedVolAggregation::UpperQuantile { .. } => Self::UpperQuantile,
            source::RealizedVolAggregation::Median => Self::Median,
            source::RealizedVolAggregation::TrimmedMean { .. } => Self::TrimmedMean,
            source::RealizedVolAggregation::MedianWithUpperQuantileGuard { .. } => {
                Self::MedianWithUpperQuantileGuard
            }
        }
    }
}

impl From<source::RealizedVolBlockReason> for RealizedVolBlockReason {
    fn from(value: source::RealizedVolBlockReason) -> Self {
        match value {
            source::RealizedVolBlockReason::InvalidConfig => Self::InvalidConfig,
            source::RealizedVolBlockReason::QuorumNotReady => Self::QuorumNotReady,
            source::RealizedVolBlockReason::SourceStale => Self::SourceStale,
            source::RealizedVolBlockReason::CoverageBelowMinimum => Self::CoverageBelowMinimum,
            source::RealizedVolBlockReason::InterSampleGapExceeded => Self::InterSampleGapExceeded,
            source::RealizedVolBlockReason::SourceClassMismatch => Self::SourceClassMismatch,
            source::RealizedVolBlockReason::SampleKindMismatch => Self::SampleKindMismatch,
            source::RealizedVolBlockReason::CrossSourceDispersion => Self::CrossSourceDispersion,
            source::RealizedVolBlockReason::AnnualizationBasisInvalid => {
                Self::AnnualizationBasisInvalid
            }
            source::RealizedVolBlockReason::NotWarm => Self::NotWarm,
        }
    }
}

impl From<source::RealizedVolSourceClass> for RealizedVolSourceClass {
    fn from(value: source::RealizedVolSourceClass) -> Self {
        match value {
            source::RealizedVolSourceClass::SpotQuote => Self::SpotQuote,
            source::RealizedVolSourceClass::Trade => Self::Trade,
            source::RealizedVolSourceClass::Mark => Self::Mark,
            source::RealizedVolSourceClass::Index => Self::Index,
        }
    }
}

impl From<source::RealizedVolSampleKind> for RealizedVolSampleKind {
    fn from(value: source::RealizedVolSampleKind) -> Self {
        match value {
            source::RealizedVolSampleKind::Midpoint => Self::Midpoint,
            source::RealizedVolSampleKind::Trade => Self::Trade,
            source::RealizedVolSampleKind::Mark => Self::Mark,
            source::RealizedVolSampleKind::Index => Self::Index,
        }
    }
}

impl From<source::RealizedVolSourceStatus> for RealizedVolSourceStatus {
    fn from(value: source::RealizedVolSourceStatus) -> Self {
        match value {
            source::RealizedVolSourceStatus::Ready => Self::Ready,
            source::RealizedVolSourceStatus::Blocked => Self::Blocked,
            source::RealizedVolSourceStatus::DiagnosticOnly => Self::DiagnosticOnly,
            source::RealizedVolSourceStatus::Waiting => Self::Waiting,
        }
    }
}

impl From<source::RealizedVolSourceRejectReason> for RealizedVolSourceRejectReason {
    fn from(value: source::RealizedVolSourceRejectReason) -> Self {
        match value {
            source::RealizedVolSourceRejectReason::DisabledSource => Self::DisabledSource,
            source::RealizedVolSourceRejectReason::InvalidPrice => Self::InvalidPrice,
            source::RealizedVolSourceRejectReason::SourceClassMismatch => Self::SourceClassMismatch,
            source::RealizedVolSourceRejectReason::SampleKindMismatch => Self::SampleKindMismatch,
            source::RealizedVolSourceRejectReason::EventTimeRegression => Self::EventTimeRegression,
            source::RealizedVolSourceRejectReason::DuplicateTimestamp => Self::DuplicateTimestamp,
            source::RealizedVolSourceRejectReason::StaleSameEventUpdate => {
                Self::StaleSameEventUpdate
            }
            source::RealizedVolSourceRejectReason::ReceiveBeforeEvent => Self::ReceiveBeforeEvent,
            source::RealizedVolSourceRejectReason::EventReceiveLagExceeded => {
                Self::EventReceiveLagExceeded
            }
        }
    }
}

pub(crate) fn source_diagnostic_fact(
    value: &source::RealizedVolSourceDiagnostic,
) -> RealizedVolatilitySourceDiagnosticFact {
    RealizedVolatilitySourceDiagnosticFact {
        source_id: value.source_id.clone(),
        source_class: value.source_class.into(),
        sample_kind: value.sample_kind.into(),
        enabled: value.enabled,
        counts_toward_quorum: value.counts_toward_quorum,
        status: value.status.into(),
        annualized_realized_volatility_decimal: value
            .annualized_realized_vol_decimal
            .map(evidence_number),
        measured_annualized_realized_volatility_decimal: value
            .measured_annualized_realized_vol_decimal
            .map(evidence_number),
        noise_robust_annualized_realized_volatility_decimal: value
            .noise_robust_annualized_realized_vol_decimal
            .map(evidence_number),
        continuous_annualized_realized_volatility_decimal: value
            .continuous_annualized_realized_vol_decimal
            .map(evidence_number),
        jump_annualized_realized_volatility_decimal: value
            .jump_annualized_realized_vol_decimal
            .map(evidence_number),
        first_sample_ts_ms: value.first_sample_ts_ms,
        last_sample_ts_ms: value.last_sample_ts_ms,
        raw_sample_count: value.raw_sample_count,
        grid_sample_count: value.grid_sample_count,
        coverage_ratio: evidence_number(value.coverage_ratio),
        max_inter_sample_gap_ms: value.max_inter_sample_gap_ms,
        last_rejected_reason: value.last_rejected_reason.map(Into::into),
        last_rejected_event_ts_ms: value.last_rejected_event_ts_ms,
        last_rejected_recv_ts_ms: value.last_rejected_recv_ts_ms,
        rejection_counters: value
            .rejection_counters
            .iter()
            .map(|(reason, count)| ((*reason).into(), *count))
            .collect(),
        block_reason: value.block_reason.map(Into::into),
    }
}
