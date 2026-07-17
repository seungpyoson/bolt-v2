use crate::economics::{
    AdmissionTreatment, CalculationFactor, EconomicClass, EconomicKind, EconomicQuoteRequest,
    EconomicScope, EconomicsUnavailable, EstimatedEconomicComponent, ExecutionKind, FormulaId,
    LiquidityRoleAssumption, NativeUnitId, SignedNativeEffect, SnapshotId, SourceId,
    SourceValidity, VenueEconomicsAdapter, VenueQuoteEstimate, basis_points_to_fraction,
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
    rates: ProtocolRatePlan,
    builder: BuilderQuotePlan,
}

struct ProtocolRatePlan {
    maker: Decimal,
    taker: Decimal,
}

enum BuilderQuotePlan {
    Unavailable,
    Approved {
        profile_id: String,
        rate_bps: Decimal,
    },
}

enum ScalePlan {
    Identity,
    Multiply(Decimal),
}

impl ScalePlan {
    fn apply(&self, rate: Decimal) -> Decimal {
        match self {
            Self::Identity => rate,
            Self::Multiply(scale) => rate * scale,
        }
    }
}

impl HyperliquidEconomicsAdapter {
    pub fn try_new(
        config: HyperliquidEconomicsAdapterConfig,
        user_fees: HyperliquidUserFeesSnapshot,
        product: HyperliquidProductEconomicsSnapshot,
    ) -> Result<Self, HyperliquidEconomicsError> {
        validate_authority_snapshots(&user_fees, &product)?;
        if product.deployer_scale < Decimal::ZERO {
            return Err(HyperliquidEconomicsError::InvalidFeeSurface);
        }
        let base_rates = match product.product_kind {
            HyperliquidProductKind::Perp => ProtocolRatePlan {
                maker: user_fees.perp_maker_base_rate,
                taker: user_fees.perp_taker_base_rate,
            },
            HyperliquidProductKind::Spot => ProtocolRatePlan {
                maker: user_fees.spot_maker_base_rate,
                taker: user_fees.spot_taker_base_rate,
            },
        };
        let stable = match product.stable_pair {
            false => ScalePlan::Identity,
            true => ScalePlan::Multiply(config.formula.stable_pair_scale),
        };
        let growth = match product.growth_mode {
            false => ScalePlan::Identity,
            true => ScalePlan::Multiply(config.formula.growth_mode_scale),
        };
        let hip3 = match (
            product.hip3,
            product.deployer_scale < config.formula.hip3_scale_threshold,
        ) {
            (false, _) => ScalePlan::Identity,
            (true, true) => ScalePlan::Multiply(
                config.formula.hip3_below_threshold_base + product.deployer_scale,
            ),
            (true, false) => ScalePlan::Multiply(
                config.formula.hip3_at_or_above_threshold_multiplier * product.deployer_scale,
            ),
        };
        let discount_scale = (Decimal::ONE - user_fees.active_referral_discount)
            * (Decimal::ONE - user_fees.active_staking_discount);
        let resolve_rate = |base| hip3.apply(growth.apply(stable.apply(base * discount_scale)));
        let rates = ProtocolRatePlan {
            maker: resolve_rate(base_rates.maker),
            taker: resolve_rate(base_rates.taker),
        };
        if config.formula.hip3_at_or_above_deployer_share < Decimal::ZERO {
            return Err(HyperliquidEconomicsError::InvalidFeeSurface);
        }
        let builder = match (
            product.builder_profile_id.as_ref(),
            product.builder_rate_bps,
            product.builder_approved_max_bps,
        ) {
            (None, None, None) => BuilderQuotePlan::Unavailable,
            (Some(profile_id), Some(rate_bps), Some(approved_max_bps))
                if rate_bps >= Decimal::ZERO && rate_bps <= approved_max_bps =>
            {
                BuilderQuotePlan::Approved {
                    profile_id: profile_id.clone(),
                    rate_bps,
                }
            }
            (Some(_), Some(_), Some(_)) => {
                return Err(HyperliquidEconomicsError::BuilderApprovalExceeded);
            }
            _ => return Err(HyperliquidEconomicsError::MissingBuilderApproval),
        };
        Ok(Self {
            config,
            user_fees,
            product,
            rates,
            builder,
        })
    }

    pub fn quote_components(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<Vec<EstimatedEconomicComponent>, HyperliquidEconomicsError> {
        self.validate(request)?;
        let rate = match request.liquidity_role {
            LiquidityRoleAssumption::GuaranteedMaker => self.rates.maker,
            LiquidityRoleAssumption::Taker => self.rates.taker,
        };
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

        match (&request.routing.attached_charge, &self.builder) {
            (None, _) => {}
            (Some(_), BuilderQuotePlan::Unavailable) => {
                return Err(HyperliquidEconomicsError::MissingBuilderApproval);
            }
            (
                Some(attachment),
                BuilderQuotePlan::Approved {
                    profile_id,
                    rate_bps,
                },
            ) => {
                if profile_id != attachment.attachment_id.as_str() {
                    return Err(HyperliquidEconomicsError::BuilderProfileMismatch);
                }
                let signed_builder_amount = -(notional * basis_points_to_fraction(*rate_bps));
                if !signed_builder_amount.is_zero() {
                    components.push(self.component(
                        request,
                        self.config.builder_component_id.clone(),
                        self.config.builder_formula_id.clone(),
                        self.config.builder_rate_factor_id.clone(),
                        *rate_bps,
                        signed_builder_amount,
                        ExecutionKind::AttachedRouting,
                    )?);
                }
            }
        }
        Ok(components)
    }

    fn validate(&self, request: &EconomicQuoteRequest) -> Result<(), HyperliquidEconomicsError> {
        if self.user_fees.account_id != request.account_id.as_str() {
            return Err(HyperliquidEconomicsError::InvalidAccountScope);
        }
        if self.user_fees.fetched_at_ns > request.requested_at_ns
            || self.user_fees.valid_until_ns < request.requested_at_ns
            || self.product.fetched_at_ns > request.requested_at_ns
            || self.product.valid_until_ns < request.requested_at_ns
        {
            return Err(HyperliquidEconomicsError::StaleSnapshot);
        }
        Ok(())
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

fn validate_authority_snapshots(
    user_fees: &HyperliquidUserFeesSnapshot,
    product: &HyperliquidProductEconomicsSnapshot,
) -> Result<(), HyperliquidEconomicsError> {
    let user_fees_valid = user_fees.source_at_ns <= user_fees.fetched_at_ns
        && user_fees.fetched_at_ns <= user_fees.valid_until_ns
        && !user_fees.fee_tier.trim().is_empty()
        && user_fees.daily_user_volume >= Decimal::ZERO
        && user_fees.trial_credits >= Decimal::ZERO
        && (Decimal::ZERO..=Decimal::ONE).contains(&user_fees.active_referral_discount)
        && (Decimal::ZERO..=Decimal::ONE).contains(&user_fees.active_staking_discount);
    let product_timeline_valid = product.source_at_ns <= product.fetched_at_ns
        && product.fetched_at_ns <= product.valid_until_ns;
    match (
        user_fees_valid,
        product_timeline_valid,
        product.aligned_quote_or_collateral,
        product.product_kind,
        product.spot_dust_authority_complete,
    ) {
        (false, _, _, _, _) => Err(HyperliquidEconomicsError::InvalidUserFees),
        (_, false, _, _, _) => Err(HyperliquidEconomicsError::InvalidProductMetadata),
        (_, _, true, _, _) => Err(HyperliquidEconomicsError::BlockedUnsupported(
            BlockedUnsupported::MissingGovernedAlignedStatusCapture,
        )),
        (_, _, _, HyperliquidProductKind::Spot, false) => {
            Err(HyperliquidEconomicsError::BlockedUnsupported(
                BlockedUnsupported::SpotDustAuthorityIncomplete,
            ))
        }
        (true, true, false, _, _) => Ok(()),
    }
}

impl VenueEconomicsAdapter for HyperliquidEconomicsAdapter {
    fn quote(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<VenueQuoteEstimate, EconomicsUnavailable> {
        let components = self.quote_components(request).map_err(|_| {
            EconomicsUnavailable::ProviderQuoteUnavailable {
                source_id: self.config.source_id.clone(),
            }
        })?;
        let snapshot_id = SnapshotId::new(self.user_fees.snapshot_id.clone()).map_err(|_| {
            EconomicsUnavailable::ProviderQuoteUnavailable {
                source_id: self.config.source_id.clone(),
            }
        })?;
        Ok(VenueQuoteEstimate {
            authority: SourceValidity {
                source_id: self.config.source_id.clone(),
                snapshot_id,
                source_at_ns: self.user_fees.source_at_ns.max(self.product.source_at_ns),
                fetched_at_ns: self.user_fees.fetched_at_ns.max(self.product.fetched_at_ns),
                valid_until_ns: self
                    .user_fees
                    .valid_until_ns
                    .min(self.product.valid_until_ns),
            },
            components,
        })
    }
}
