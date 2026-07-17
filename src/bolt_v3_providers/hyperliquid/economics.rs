use crate::economics::{
    AdmissionTreatment, CalculationFactor, CarryKind, EconomicClass, EconomicKind,
    EconomicQuoteRequest, EconomicScope, EconomicsUnavailable, EstimatedEconomicComponent,
    ExecutionKind, FormulaId, LiquidityRoleAssumption, NativeUnitId, PositionSide,
    RiskBoundAuthority, SignedNativeEffect, SnapshotId, SourceId, SourceValidity,
    VenueEconomicsAdapter, VenueQuoteEstimate, basis_points_to_fraction,
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
    pub carry: Option<HyperliquidCarryPolicy>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperliquidCarryPolicy {
    pub component_id: crate::economics::EconomicComponentId,
    pub formula_id: FormulaId,
    pub point_rate_factor_id: FormulaId,
    pub bound_rate_factor_id: FormulaId,
    pub risk_policy_id: FormulaId,
    pub stress_fixture_id: FormulaId,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HyperliquidProductKind {
    Spot,
    Perp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperliquidUserFeesSnapshot {
    snapshot_id: String,
    account_id: String,
    source_at_ns: u64,
    fetched_at_ns: u64,
    valid_until_ns: u64,
    daily_user_volume: Decimal,
    active_referral_discount: Decimal,
    active_staking_discount: Decimal,
    trial_credits: Decimal,
    perp_taker_rate: Decimal,
    perp_maker_rate: Decimal,
    spot_taker_rate: Decimal,
    spot_maker_rate: Decimal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperliquidSnapshotMetadata {
    pub snapshot_id: String,
    pub source_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HyperliquidUserFeesWire {
    daily_user_vlm: Vec<HyperliquidDailyUserVolumeWire>,
    fee_schedule: HyperliquidFeeScheduleWire,
    user_cross_rate: Decimal,
    user_add_rate: Decimal,
    user_spot_cross_rate: Decimal,
    user_spot_add_rate: Decimal,
    active_referral_discount: Decimal,
    trial: Option<serde_json::Value>,
    fee_trial_reward: Decimal,
    #[serde(rename = "nextTrialAvailableTimestamp")]
    _next_trial_available_timestamp: Option<u64>,
    staking_link: Option<HyperliquidStakingLinkWire>,
    active_staking_discount: HyperliquidStakingDiscountWire,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HyperliquidDailyUserVolumeWire {
    date: String,
    user_cross: Decimal,
    user_add: Decimal,
    exchange: Decimal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HyperliquidFeeScheduleWire {
    cross: Decimal,
    add: Decimal,
    spot_cross: Decimal,
    spot_add: Decimal,
    tiers: HyperliquidFeeTiersWire,
    referral_discount: Decimal,
    staking_discount_tiers: Vec<HyperliquidStakingDiscountWire>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct HyperliquidFeeTiersWire {
    vip: Vec<HyperliquidVipFeeTierWire>,
    mm: Vec<HyperliquidMakerFeeTierWire>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HyperliquidVipFeeTierWire {
    ntl_cutoff: Decimal,
    cross: Decimal,
    add: Decimal,
    spot_cross: Decimal,
    spot_add: Decimal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HyperliquidMakerFeeTierWire {
    maker_fraction_cutoff: Decimal,
    add: Decimal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HyperliquidStakingDiscountWire {
    bps_of_max_supply: Decimal,
    discount: Decimal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct HyperliquidStakingLinkWire {
    r#type: String,
    staking_user: String,
}

impl HyperliquidUserFeesSnapshot {
    pub fn from_wire_json(
        metadata: HyperliquidSnapshotMetadata,
        account_id: &str,
        json: &str,
    ) -> Result<Self, HyperliquidEconomicsError> {
        let wire: HyperliquidUserFeesWire =
            serde_json::from_str(json).map_err(|_| HyperliquidEconomicsError::InvalidUserFees)?;
        if metadata.snapshot_id.trim().is_empty()
            || account_id.trim().is_empty()
            || wire.trial.is_some()
            || wire.daily_user_vlm.is_empty()
            || wire.daily_user_vlm.iter().any(|volume| {
                volume.date.trim().is_empty()
                    || volume.user_cross < Decimal::ZERO
                    || volume.user_add < Decimal::ZERO
                    || volume.exchange < Decimal::ZERO
            })
            || !valid_fee_schedule(&wire)
        {
            return Err(HyperliquidEconomicsError::InvalidUserFees);
        }
        let daily_user_volume = wire
            .daily_user_vlm
            .iter()
            .try_fold(Decimal::ZERO, |total, volume| {
                total
                    .checked_add(volume.user_cross)
                    .and_then(|value| value.checked_add(volume.user_add))
            })
            .ok_or(HyperliquidEconomicsError::InvalidUserFees)?;
        Ok(Self {
            snapshot_id: metadata.snapshot_id,
            account_id: account_id.to_string(),
            source_at_ns: metadata.source_at_ns,
            fetched_at_ns: metadata.fetched_at_ns,
            valid_until_ns: metadata.valid_until_ns,
            daily_user_volume,
            active_referral_discount: wire.active_referral_discount,
            active_staking_discount: wire.active_staking_discount.discount,
            trial_credits: wire.fee_trial_reward,
            perp_taker_rate: wire.user_cross_rate,
            perp_maker_rate: wire.user_add_rate,
            spot_taker_rate: wire.user_spot_cross_rate,
            spot_maker_rate: wire.user_spot_add_rate,
        })
    }
}

fn valid_fee_schedule(wire: &HyperliquidUserFeesWire) -> bool {
    let schedule = &wire.fee_schedule;
    let unit_interval = |value: Decimal| (Decimal::ZERO..=Decimal::ONE).contains(&value);
    let nonnegative_rates = schedule.cross >= Decimal::ZERO
        && schedule.add >= Decimal::ZERO
        && schedule.spot_cross >= Decimal::ZERO
        && schedule.spot_add >= Decimal::ZERO
        && wire.user_cross_rate >= Decimal::ZERO
        && wire.user_spot_cross_rate >= Decimal::ZERO;
    let effective_within_base = wire.user_cross_rate <= schedule.cross
        && wire.user_spot_cross_rate <= schedule.spot_cross
        && wire.user_add_rate <= schedule.add
        && wire.user_spot_add_rate <= schedule.spot_add;
    let tier_rates_valid = schedule.vip.iter().all(|tier| {
        tier.ntl_cutoff >= Decimal::ZERO
            && tier.cross >= Decimal::ZERO
            && tier.add >= Decimal::ZERO
            && tier.spot_cross >= Decimal::ZERO
            && tier.spot_add >= Decimal::ZERO
    }) && schedule
        .mm
        .iter()
        .all(|tier| tier.maker_fraction_cutoff >= Decimal::ZERO && tier.add <= schedule.add);
    let staking_valid = !schedule.staking_discount_tiers.is_empty()
        && schedule
            .staking_discount_tiers
            .iter()
            .all(|tier| tier.bps_of_max_supply >= Decimal::ZERO && unit_interval(tier.discount))
        && schedule
            .staking_discount_tiers
            .iter()
            .any(|tier| tier.discount == wire.active_staking_discount.discount);
    let staking_link_valid = wire
        .staking_link
        .as_ref()
        .is_none_or(|link| !link.r#type.trim().is_empty() && !link.staking_user.trim().is_empty());
    nonnegative_rates
        && effective_within_base
        && tier_rates_valid
        && staking_valid
        && staking_link_valid
        && wire.fee_trial_reward >= Decimal::ZERO
        && unit_interval(schedule.referral_discount)
        && unit_interval(wire.active_referral_discount)
        && wire.active_referral_discount <= schedule.referral_discount
        && wire.active_staking_discount.bps_of_max_supply >= Decimal::ZERO
        && unit_interval(wire.active_staking_discount.discount)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct HyperliquidProductEconomicsSnapshot {
    snapshot_id: String,
    source_at_ns: u64,
    fetched_at_ns: u64,
    valid_until_ns: u64,
    product_kind: HyperliquidProductKind,
    base_unit: Option<String>,
    quote_unit: Option<String>,
    stable_pair: bool,
    aligned_quote_or_collateral: bool,
    hip3: bool,
    deployer_scale: Decimal,
    growth_mode: bool,
    builder_profile_id: Option<String>,
    builder_rate_bps: Option<Decimal>,
    builder_approved_max_bps: Option<Decimal>,
    spot_dust_authority_complete: bool,
    carry_point_rate_per_ns: Option<Decimal>,
    carry_debit_rate_bound_per_ns: Option<Decimal>,
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
    MissingSpotUnit,
    MissingCarryPolicy,
    MissingCarryContext,
    InvalidCarryBound,
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
                maker: user_fees.perp_maker_rate,
                taker: user_fees.perp_taker_rate,
            },
            HyperliquidProductKind::Spot => ProtocolRatePlan {
                maker: user_fees.spot_maker_rate,
                taker: user_fees.spot_taker_rate,
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
        let referral_scale = Decimal::ONE - user_fees.active_referral_discount;
        let maker_before_product_and_referral = growth.apply(stable.apply(base_rates.maker));
        let rates = ProtocolRatePlan {
            maker: if maker_before_product_and_referral > Decimal::ZERO {
                hip3.apply(maker_before_product_and_referral) * referral_scale
            } else {
                maker_before_product_and_referral
            },
            taker: hip3.apply(growth.apply(stable.apply(base_rates.taker))) * referral_scale,
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
        let (protocol_basis, protocol_unit) = match (self.product.product_kind, request.order_side)
        {
            (HyperliquidProductKind::Spot, crate::economics::OrderSide::Buy) => (
                self.quantity(request)?,
                NativeUnitId::new(
                    self.product
                        .base_unit
                        .as_ref()
                        .ok_or(HyperliquidEconomicsError::MissingSpotUnit)?
                        .clone(),
                )
                .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
            ),
            (HyperliquidProductKind::Spot, crate::economics::OrderSide::Sell) => (
                notional,
                NativeUnitId::new(
                    self.product
                        .quote_unit
                        .as_ref()
                        .ok_or(HyperliquidEconomicsError::MissingSpotUnit)?
                        .clone(),
                )
                .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
            ),
            (HyperliquidProductKind::Perp, _) => (notional, self.config.settlement_unit.clone()),
        };
        let signed_protocol_amount = -(protocol_basis * rate);
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
                protocol_unit.clone(),
            )?);
        }

        let builder_applies = !matches!(
            (self.product.product_kind, request.order_side),
            (
                HyperliquidProductKind::Spot,
                crate::economics::OrderSide::Buy
            )
        );
        match (
            &request.routing.attached_charge,
            &self.builder,
            builder_applies,
        ) {
            (_, _, false) => {}
            (None, _, true) => {}
            (Some(_), BuilderQuotePlan::Unavailable, true) => {
                return Err(HyperliquidEconomicsError::MissingBuilderApproval);
            }
            (
                Some(attachment),
                BuilderQuotePlan::Approved {
                    profile_id,
                    rate_bps,
                },
                true,
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
                        protocol_unit,
                    )?);
                }
            }
        }
        if self.product.product_kind == HyperliquidProductKind::Perp {
            components.push(self.carry_component(request)?);
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

    fn quantity(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<Decimal, HyperliquidEconomicsError> {
        request
            .planned_fill_legs
            .iter()
            .try_fold(Decimal::ZERO, |total, leg| {
                if leg.quantity <= Decimal::ZERO {
                    return Err(HyperliquidEconomicsError::InvalidFillLeg);
                }
                total
                    .checked_add(leg.quantity)
                    .ok_or(HyperliquidEconomicsError::InvalidFillLeg)
            })
    }

    fn carry_component(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<EstimatedEconomicComponent, HyperliquidEconomicsError> {
        let policy = self
            .config
            .carry
            .as_ref()
            .ok_or(HyperliquidEconomicsError::MissingCarryPolicy)?;
        let position = request
            .position
            .as_ref()
            .ok_or(HyperliquidEconomicsError::MissingCarryContext)?;
        if position.quantity <= Decimal::ZERO || position.holding_horizon_ns == 0 {
            return Err(HyperliquidEconomicsError::MissingCarryContext);
        }
        let point_rate = self
            .product
            .carry_point_rate_per_ns
            .ok_or(HyperliquidEconomicsError::MissingCarryPolicy)?;
        let bound_rate = self
            .product
            .carry_debit_rate_bound_per_ns
            .ok_or(HyperliquidEconomicsError::MissingCarryPolicy)?;
        if bound_rate <= Decimal::ZERO || point_rate.is_zero() {
            return Err(HyperliquidEconomicsError::InvalidCarryBound);
        }
        let average_price = self
            .notional(request)?
            .checked_div(self.quantity(request)?)
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        let horizon = Decimal::from(position.holding_horizon_ns);
        let position_notional = position
            .quantity
            .checked_mul(average_price)
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        let directional_point_rate = match position.side {
            PositionSide::Long => -point_rate,
            PositionSide::Short => point_rate,
        };
        let point_amount = position_notional
            .checked_mul(directional_point_rate)
            .and_then(|amount| amount.checked_mul(horizon))
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        let debit_bound = position_notional
            .checked_mul(-bound_rate)
            .and_then(|amount| amount.checked_mul(horizon))
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        if debit_bound >= Decimal::ZERO {
            return Err(HyperliquidEconomicsError::InvalidCarryBound);
        }
        Ok(EstimatedEconomicComponent {
            component_id: policy.component_id.clone(),
            class: if point_amount.is_sign_negative() {
                EconomicClass::Charge
            } else {
                EconomicClass::Credit
            },
            kind: EconomicKind::Carry(CarryKind::Funding),
            scope: EconomicScope::Decision {
                decision_correlation_id: request.decision_correlation_id.clone(),
            },
            point_effect: SignedNativeEffect::currency(
                point_amount,
                self.config.settlement_unit.clone(),
            )
            .map_err(|_| HyperliquidEconomicsError::InvalidEffect)?,
            debit_risk_bound: Some(
                SignedNativeEffect::currency(debit_bound, self.config.settlement_unit.clone())
                    .map_err(|_| HyperliquidEconomicsError::InvalidEffect)?,
            ),
            admission_treatment: AdmissionTreatment::RiskBound {
                authority: RiskBoundAuthority::OperatorRiskLimit,
            },
            calculation_factors: vec![
                CalculationFactor {
                    factor_id: policy.point_rate_factor_id.clone(),
                    value: point_rate,
                },
                CalculationFactor {
                    factor_id: policy.bound_rate_factor_id.clone(),
                    value: bound_rate,
                },
                CalculationFactor {
                    factor_id: policy.risk_policy_id.clone(),
                    value: horizon,
                },
                CalculationFactor {
                    factor_id: policy.stress_fixture_id.clone(),
                    value: average_price,
                },
            ],
            formula_id: policy.formula_id.clone(),
            source: SourceValidity {
                source_id: self.config.source_id.clone(),
                snapshot_id: SnapshotId::new(self.product.snapshot_id.clone())
                    .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
                source_at_ns: self.product.source_at_ns,
                fetched_at_ns: self.product.fetched_at_ns,
                valid_until_ns: self.product.valid_until_ns,
            },
            normalized: None,
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
        native_unit: NativeUnitId,
    ) -> Result<EstimatedEconomicComponent, HyperliquidEconomicsError> {
        let class = if amount.is_sign_negative() {
            EconomicClass::Charge
        } else {
            EconomicClass::Credit
        };
        let point_effect = SignedNativeEffect::currency(amount, native_unit)
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
        && user_fees.daily_user_volume >= Decimal::ZERO
        && user_fees.trial_credits >= Decimal::ZERO
        && (Decimal::ZERO..=Decimal::ONE).contains(&user_fees.active_referral_discount)
        && (Decimal::ZERO..=Decimal::ONE).contains(&user_fees.active_staking_discount)
        && user_fees.perp_taker_rate >= Decimal::ZERO
        && user_fees.spot_taker_rate >= Decimal::ZERO;
    let product_timeline_valid = product.source_at_ns <= product.fetched_at_ns
        && product.fetched_at_ns <= product.valid_until_ns;
    let product_units_valid = match product.product_kind {
        HyperliquidProductKind::Spot => {
            product
                .base_unit
                .as_ref()
                .is_some_and(|unit| !unit.trim().is_empty())
                && product
                    .quote_unit
                    .as_ref()
                    .is_some_and(|unit| !unit.trim().is_empty())
        }
        HyperliquidProductKind::Perp => true,
    };
    match (
        user_fees_valid,
        product_timeline_valid,
        product_units_valid,
        product.aligned_quote_or_collateral,
        product.product_kind,
        product.spot_dust_authority_complete,
    ) {
        (false, _, _, _, _, _) => Err(HyperliquidEconomicsError::InvalidUserFees),
        (_, false, _, _, _, _) => Err(HyperliquidEconomicsError::InvalidProductMetadata),
        (_, _, false, _, _, _) => Err(HyperliquidEconomicsError::MissingSpotUnit),
        (_, _, _, true, _, _) => Err(HyperliquidEconomicsError::BlockedUnsupported(
            BlockedUnsupported::MissingGovernedAlignedStatusCapture,
        )),
        (_, _, _, _, HyperliquidProductKind::Spot, false) => {
            Err(HyperliquidEconomicsError::BlockedUnsupported(
                BlockedUnsupported::SpotDustAuthorityIncomplete,
            ))
        }
        (true, true, true, false, _, _) => Ok(()),
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
        let product_snapshot_id =
            SnapshotId::new(self.product.snapshot_id.clone()).map_err(|_| {
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
            dependency_sources: vec![SourceValidity {
                source_id: self.config.source_id.clone(),
                snapshot_id: product_snapshot_id,
                source_at_ns: self.product.source_at_ns,
                fetched_at_ns: self.product.fetched_at_ns,
                valid_until_ns: self.product.valid_until_ns,
            }],
            components,
        })
    }
}
