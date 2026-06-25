use rust_decimal::Decimal;

use crate::bolt_v3_risk_reservation_substrate::{
    contracts::{
        ActiveDescriptorView, AdmissionCandidate, RiskAssessment, RiskPreviewInput, RiskSizingView,
    },
    risk_classifier::{
        RiskClassification, RiskClassificationError, RiskClassificationPolicy, RiskClassifier,
        RiskDescriptorCanonicalAttributes,
    },
    risk_kernel::{
        RiskCandidate, RiskEvaluationScope, RiskKernel, RiskKernelError, RiskKernelInput,
        RiskPortfolioSnapshot,
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskViewPublisher;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RiskViewPublicationInput {
    pub sizing_view: RiskSizingView,
    pub active_descriptor: ActiveDescriptorView,
    pub descriptor_attributes: RiskDescriptorCanonicalAttributes,
    pub classification_policy: RiskClassificationPolicy,
    pub caller_declared_buckets:
        Vec<crate::bolt_v3_risk_reservation_substrate::risk_classifier::ConcentrationBucket>,
    pub portfolio: RiskPortfolioSnapshot,
    pub portfolio_scope_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedRiskView {
    sizing_view: RiskSizingView,
    active_descriptor: ActiveDescriptorView,
    classification: RiskClassification,
    portfolio: RiskPortfolioSnapshot,
    portfolio_scope_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskViewPublishError {
    InvalidSizingView,
    InvalidActiveDescriptor,
    Classification(RiskClassificationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskPreviewError {
    StaleRiskStateVersion,
    PolicyEpochMismatch,
    InstrumentMismatch,
    DescriptorVersionMismatch,
    InvalidCandidate,
    Kernel(RiskKernelError),
}

impl RiskViewPublisher {
    pub fn publish(
        input: RiskViewPublicationInput,
    ) -> Result<PublishedRiskView, RiskViewPublishError> {
        validate_sizing_view(&input.sizing_view)?;
        validate_active_descriptor(&input.active_descriptor)?;

        let classification = RiskClassifier::classify(
            &input.descriptor_attributes,
            &input.classification_policy,
            &input.caller_declared_buckets,
        )
        .map_err(RiskViewPublishError::Classification)?;

        Ok(PublishedRiskView {
            sizing_view: input.sizing_view,
            active_descriptor: input.active_descriptor,
            classification,
            portfolio: input.portfolio,
            portfolio_scope_id: input.portfolio_scope_id,
        })
    }

    pub fn preview(
        view: &PublishedRiskView,
        input: &RiskPreviewInput,
    ) -> Result<RiskAssessment, RiskPreviewError> {
        RiskKernel::evaluate(&view.kernel_input_for_preview(input)?)
            .map_err(RiskPreviewError::Kernel)
    }
}

impl PublishedRiskView {
    pub fn sizing_view(&self) -> &RiskSizingView {
        &self.sizing_view
    }

    pub fn active_descriptor(&self) -> &ActiveDescriptorView {
        &self.active_descriptor
    }

    pub fn kernel_input_for_preview(
        &self,
        input: &RiskPreviewInput,
    ) -> Result<RiskKernelInput, RiskPreviewError> {
        if input.source_view_version != self.sizing_view.risk_state_version {
            return Err(RiskPreviewError::StaleRiskStateVersion);
        }
        if input.policy_epoch_id != self.active_descriptor.policy_epoch_id {
            return Err(RiskPreviewError::PolicyEpochMismatch);
        }
        if input.instrument_id != self.active_descriptor.instrument_id {
            return Err(RiskPreviewError::InstrumentMismatch);
        }

        self.kernel_input(
            input.instrument_id.clone(),
            input.model_risk_scope.clone(),
            input.quantity,
            input.max_cash_outlay,
            input.source_view_version,
        )
    }

    pub fn kernel_input_for_candidate(
        &self,
        candidate: &AdmissionCandidate,
    ) -> Result<RiskKernelInput, RiskPreviewError> {
        if candidate.source_view_version != self.sizing_view.risk_state_version {
            return Err(RiskPreviewError::StaleRiskStateVersion);
        }
        if candidate.policy_epoch_id != self.active_descriptor.policy_epoch_id {
            return Err(RiskPreviewError::PolicyEpochMismatch);
        }
        if candidate.instrument_id != self.active_descriptor.instrument_id {
            return Err(RiskPreviewError::InstrumentMismatch);
        }
        if candidate.expected_descriptor_version != self.active_descriptor.descriptor_version {
            return Err(RiskPreviewError::DescriptorVersionMismatch);
        }

        self.kernel_input(
            candidate.instrument_id.clone(),
            candidate.model_risk_scope.clone(),
            candidate.quantity,
            candidate.max_cash_outlay,
            candidate.source_view_version,
        )
    }

    fn kernel_input(
        &self,
        instrument_id: String,
        model_risk_scope: crate::bolt_v3_risk_reservation_substrate::contracts::ModelRiskEvaluationScope,
        quantity: Decimal,
        max_cash_outlay: Decimal,
        risk_state_version: crate::bolt_v3_risk_reservation_substrate::contracts::RiskStateVersion,
    ) -> Result<RiskKernelInput, RiskPreviewError> {
        if quantity <= Decimal::ZERO || max_cash_outlay < Decimal::ZERO {
            return Err(RiskPreviewError::InvalidCandidate);
        }
        Ok(RiskKernelInput {
            risk_state_version,
            portfolio: self.portfolio.clone(),
            candidate: RiskCandidate {
                instrument_id,
                buckets: self.classification.buckets().clone(),
                quantity,
                conservative_liquidation_value: max_cash_outlay,
                governor_cost_basis: max_cash_outlay,
                terminal_cash_flows: self.active_descriptor.terminal_cash_flows.clone(),
            },
            evaluation_scope: RiskEvaluationScope::from(&model_risk_scope),
            portfolio_scope_id: self.portfolio_scope_id.clone(),
        })
    }
}

fn validate_sizing_view(view: &RiskSizingView) -> Result<(), RiskViewPublishError> {
    if !view.reconciliation_ready
        || view.reference_growth_wealth > view.conservative_liquidation_equity
        || view.reference_growth_wealth < Decimal::ZERO
        || view.conservative_liquidation_equity < Decimal::ZERO
        || view.free_collateral < Decimal::ZERO
        || view.equity_floor_headroom < Decimal::ZERO
        || view.governor_headroom < Decimal::ZERO
        || view.global_stress_loss_headroom < Decimal::ZERO
        || view.position_quantity_headroom < Decimal::ZERO
        || view
            .bucket_stress_loss_headrooms
            .values()
            .any(|headroom| *headroom < Decimal::ZERO)
    {
        return Err(RiskViewPublishError::InvalidSizingView);
    }
    Ok(())
}

fn validate_active_descriptor(
    descriptor: &ActiveDescriptorView,
) -> Result<(), RiskViewPublishError> {
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
        return Err(RiskViewPublishError::InvalidActiveDescriptor);
    }
    Ok(())
}

fn is_clean_runtime_value(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}
