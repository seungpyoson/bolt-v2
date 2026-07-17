use crate::economics::{
    AdmissionTreatment, CalculationFactor, EconomicClass, EconomicKind, EconomicQuoteRequest,
    EconomicScope, EconomicsUnavailable, EstimatedEconomicComponent, ExecutionKind, FormulaId,
    LiquidityRoleAssumption, NativeUnitId, SignedNativeEffect, SnapshotId, SourceId,
    SourceValidity, VenueEconomicsAdapter, basis_points_to_fraction,
};
use rust_decimal::Decimal;
use serde::Deserialize;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperliquidFormulaPolicy {
    pub stable_pair_scale: Decimal,
    pub growth_mode_scale: Decimal,
    pub hip3_scale_threshold: Decimal,
    pub hip3_below_threshold_base: Decimal,
    pub hip3_at_or_above_threshold_multiplier: Decimal,
    pub hip3_at_or_above_deployer_share: Decimal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperliquidEconomicsAdapterConfig {
    pub settlement_unit: NativeUnitId,
    pub protocol_component_id: crate::economics::EconomicComponentId,
    pub protocol_formula_id: FormulaId,
    pub protocol_rate_factor_id: FormulaId,
    pub builder_component_id: crate::economics::EconomicComponentId,
    pub builder_formula_id: FormulaId,
    pub builder_rate_factor_id: FormulaId,
    pub source_id: SourceId,
    pub formula: HyperliquidFormulaPolicy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HyperliquidProductKind {
    Spot,
    Perp,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HyperliquidUserFeesSnapshot {
    snapshot_id: String,
    account_id: String,
    source_at_ns: u64,
    fetched_at_ns: u64,
    valid_until_ns: u64,
    fee_tier: String,
    daily_user_volume: Decimal,
    active_referral_discount: Decimal,
    active_staking_discount: Decimal,
    trial_credits: Decimal,
    perp_taker_base_rate: Decimal,
    perp_maker_base_rate: Decimal,
    spot_taker_base_rate: Decimal,
    spot_maker_base_rate: Decimal,
}

impl HyperliquidUserFeesSnapshot {
    pub fn from_json(json: &str) -> Result<Self, HyperliquidEconomicsError> {
        serde_json::from_str(json).map_err(|_| HyperliquidEconomicsError::InvalidUserFees)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HyperliquidProductEconomicsSnapshot {
    snapshot_id: String,
    source_at_ns: u64,
    fetched_at_ns: u64,
    valid_until_ns: u64,
    product_kind: HyperliquidProductKind,
    stable_pair: bool,
    aligned_quote_or_collateral: bool,
    hip3: bool,
    deployer_scale: Decimal,
    growth_mode: bool,
    builder_profile_id: Option<String>,
    builder_rate_bps: Option<Decimal>,
    builder_approved_max_bps: Option<Decimal>,
    spot_dust_authority_complete: bool,
}

impl HyperliquidProductEconomicsSnapshot {
    pub fn from_json(json: &str) -> Result<Self, HyperliquidEconomicsError> {
        serde_json::from_str(json).map_err(|_| HyperliquidEconomicsError::InvalidProductMetadata)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockedUnsupported {
    MissingGovernedAlignedStatusCapture,
    SpotDustAuthorityIncomplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HyperliquidEconomicsError {
    InvalidUserFees,
    InvalidProductMetadata,
    InvalidAccountScope,
    InvalidFeeSurface,
    StaleSnapshot,
    BlockedUnsupported(BlockedUnsupported),
    MissingBuilderApproval,
    BuilderApprovalExceeded,
    BuilderProfileMismatch,
    InvalidFillLeg,
    InvalidIdentity,
    InvalidEffect,
}

pub struct HyperliquidEconomicsAdapter {
    config: HyperliquidEconomicsAdapterConfig,
    user_fees: HyperliquidUserFeesSnapshot,
    product: HyperliquidProductEconomicsSnapshot,
}

impl HyperliquidEconomicsAdapter {
    pub fn new(
        config: HyperliquidEconomicsAdapterConfig,
        user_fees: HyperliquidUserFeesSnapshot,
        product: HyperliquidProductEconomicsSnapshot,
    ) -> Self {
        Self {
            config,
            user_fees,
            product,
        }
    }

    pub fn quote_components(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<Vec<EstimatedEconomicComponent>, HyperliquidEconomicsError> {
        self.validate(request)?;
        let rate = self.effective_protocol_rate(request)?;
        let notional = self.notional(request)?;
        let signed_protocol_amount = -(notional * rate);
        let mut components = Vec::new();
        if !signed_protocol_amount.is_zero() {
            components.push(self.component(
                request,
                self.config.protocol_component_id.clone(),
                self.config.protocol_formula_id.clone(),
                self.config.protocol_rate_factor_id.clone(),
                rate,
                signed_protocol_amount,
                ExecutionKind::ProtocolTrading,
            )?);
        }

        if let Some(attachment) = &request.routing.attached_charge {
            let profile_id = self
                .product
                .builder_profile_id
                .as_deref()
                .ok_or(HyperliquidEconomicsError::MissingBuilderApproval)?;
            if profile_id != attachment.attachment_id.as_str() {
                return Err(HyperliquidEconomicsError::BuilderProfileMismatch);
            }
            let rate_bps = self
                .product
                .builder_rate_bps
                .ok_or(HyperliquidEconomicsError::MissingBuilderApproval)?;
            let approved_max_bps = self
                .product
                .builder_approved_max_bps
                .ok_or(HyperliquidEconomicsError::MissingBuilderApproval)?;
            if rate_bps < Decimal::ZERO || rate_bps > approved_max_bps {
                return Err(HyperliquidEconomicsError::BuilderApprovalExceeded);
            }
            let signed_builder_amount = -(notional * basis_points_to_fraction(rate_bps));
            if !signed_builder_amount.is_zero() {
                components.push(self.component(
                    request,
                    self.config.builder_component_id.clone(),
                    self.config.builder_formula_id.clone(),
                    self.config.builder_rate_factor_id.clone(),
                    rate_bps,
                    signed_builder_amount,
                    ExecutionKind::AttachedRouting,
                )?);
            }
        }
        Ok(components)
    }

    fn validate(&self, request: &EconomicQuoteRequest) -> Result<(), HyperliquidEconomicsError> {
        if self.user_fees.account_id != request.account_id.as_str() {
            return Err(HyperliquidEconomicsError::InvalidAccountScope);
        }
        if self.user_fees.source_at_ns > self.user_fees.fetched_at_ns
            || self.user_fees.fetched_at_ns > request.requested_at_ns
            || self.user_fees.valid_until_ns < request.requested_at_ns
            || self.product.source_at_ns > self.product.fetched_at_ns
            || self.product.fetched_at_ns > request.requested_at_ns
            || self.product.valid_until_ns < request.requested_at_ns
        {
            return Err(HyperliquidEconomicsError::StaleSnapshot);
        }
        if self.product.aligned_quote_or_collateral {
            return Err(HyperliquidEconomicsError::BlockedUnsupported(
                BlockedUnsupported::MissingGovernedAlignedStatusCapture,
            ));
        }
        if self.product.product_kind == HyperliquidProductKind::Spot
            && !self.product.spot_dust_authority_complete
        {
            return Err(HyperliquidEconomicsError::BlockedUnsupported(
                BlockedUnsupported::SpotDustAuthorityIncomplete,
            ));
        }
        let discounts = [
            self.user_fees.active_referral_discount,
            self.user_fees.active_staking_discount,
        ];
        if self.user_fees.fee_tier.trim().is_empty()
            || self.user_fees.daily_user_volume < Decimal::ZERO
            || self.user_fees.trial_credits < Decimal::ZERO
            || discounts
                .into_iter()
                .any(|discount| discount < Decimal::ZERO || discount > Decimal::ONE)
        {
            return Err(HyperliquidEconomicsError::InvalidUserFees);
        }
        Ok(())
    }

    fn effective_protocol_rate(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<Decimal, HyperliquidEconomicsError> {
        let base = match (self.product.product_kind, request.liquidity_role) {
            (HyperliquidProductKind::Perp, LiquidityRoleAssumption::Taker) => {
                self.user_fees.perp_taker_base_rate
            }
            (HyperliquidProductKind::Perp, LiquidityRoleAssumption::GuaranteedMaker) => {
                self.user_fees.perp_maker_base_rate
            }
            (HyperliquidProductKind::Spot, LiquidityRoleAssumption::Taker) => {
                self.user_fees.spot_taker_base_rate
            }
            (HyperliquidProductKind::Spot, LiquidityRoleAssumption::GuaranteedMaker) => {
                self.user_fees.spot_maker_base_rate
            }
        };
        let mut rate = base
            * (Decimal::ONE - self.user_fees.active_referral_discount)
            * (Decimal::ONE - self.user_fees.active_staking_discount);
        if self.product.stable_pair {
            rate *= self.config.formula.stable_pair_scale;
        }
        if self.product.growth_mode {
            rate *= self.config.formula.growth_mode_scale;
        }
        if self.product.hip3 {
            let hip3_scale =
                if self.product.deployer_scale < self.config.formula.hip3_scale_threshold {
                    self.config.formula.hip3_below_threshold_base + self.product.deployer_scale
                } else {
                    self.config.formula.hip3_at_or_above_threshold_multiplier
                        * self.product.deployer_scale
                };
            if self.config.formula.hip3_at_or_above_deployer_share < Decimal::ZERO
                || hip3_scale < Decimal::ZERO
            {
                return Err(HyperliquidEconomicsError::InvalidFeeSurface);
            }
            rate *= hip3_scale;
        }
        Ok(rate)
    }

    fn notional(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<Decimal, HyperliquidEconomicsError> {
        request
            .planned_fill_legs
            .iter()
            .try_fold(Decimal::ZERO, |total, leg| {
                if leg.price <= Decimal::ZERO || leg.quantity <= Decimal::ZERO {
                    return Err(HyperliquidEconomicsError::InvalidFillLeg);
                }
                Ok(total + leg.price * leg.quantity)
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn component(
        &self,
        request: &EconomicQuoteRequest,
        component_id: crate::economics::EconomicComponentId,
        formula_id: FormulaId,
        factor_id: FormulaId,
        factor_value: Decimal,
        amount: Decimal,
        kind: ExecutionKind,
    ) -> Result<EstimatedEconomicComponent, HyperliquidEconomicsError> {
        let class = if amount.is_sign_negative() {
            EconomicClass::Charge
        } else {
            EconomicClass::Credit
        };
        let point_effect =
            SignedNativeEffect::currency(amount, self.config.settlement_unit.clone())
                .map_err(|_| HyperliquidEconomicsError::InvalidEffect)?;
        Ok(EstimatedEconomicComponent {
            component_id,
            class,
            kind: EconomicKind::Execution(kind),
            scope: EconomicScope::Decision {
                decision_correlation_id: request.decision_correlation_id.clone(),
            },
            point_effect,
            debit_risk_bound: None,
            admission_treatment: AdmissionTreatment::GuaranteedConditionalOnAction,
            calculation_factors: vec![CalculationFactor {
                factor_id,
                value: factor_value,
            }],
            formula_id,
            source: SourceValidity {
                source_id: self.config.source_id.clone(),
                snapshot_id: SnapshotId::new(self.user_fees.snapshot_id.clone())
                    .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
                source_at_ns: self.user_fees.source_at_ns.max(self.product.source_at_ns),
                fetched_at_ns: self.user_fees.fetched_at_ns.max(self.product.fetched_at_ns),
                valid_until_ns: self
                    .user_fees
                    .valid_until_ns
                    .min(self.product.valid_until_ns),
            },
            normalized: None,
        })
    }
}

impl VenueEconomicsAdapter for HyperliquidEconomicsAdapter {
    fn quote(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<Vec<EstimatedEconomicComponent>, EconomicsUnavailable> {
        self.quote_components(request)
            .map_err(|_| EconomicsUnavailable::ProviderQuoteUnavailable {
                source_id: self.config.source_id.clone(),
            })
    }
}
