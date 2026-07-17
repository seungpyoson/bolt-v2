use crate::economics::{
    AdmissionTreatment, CalculationFactor, EconomicClass, EconomicKind, EconomicQuoteRequest,
    EconomicScope, EconomicsUnavailable, EstimatedEconomicComponent, ExecutionKind, FormulaId,
    LiquidityRoleAssumption, NativeUnitId, SignedNativeEffect, SnapshotId, SourceId,
    SourceValidity, VenueEconomicsAdapter, VenueQuoteEstimate, basis_points_to_fraction,
};
use rust_decimal::{Decimal, RoundingStrategy};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeeRoundingMode {
    MidpointAwayFromZero,
    ToZero,
}

impl FeeRoundingMode {
    fn strategy(self) -> RoundingStrategy {
        match self {
            Self::MidpointAwayFromZero => RoundingStrategy::MidpointAwayFromZero,
            Self::ToZero => RoundingStrategy::ToZero,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolymarketFormulaPolicy {
    pub fee_round_decimal_places: u32,
    pub fee_rounding_mode: FeeRoundingMode,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolymarketEconomicsAdapterConfig {
    pub collateral_unit: NativeUnitId,
    pub platform_component_id: crate::economics::EconomicComponentId,
    pub platform_formula_id: FormulaId,
    pub platform_rate_factor_id: FormulaId,
    pub builder_component_id: crate::economics::EconomicComponentId,
    pub builder_formula_id: FormulaId,
    pub builder_rate_factor_id: FormulaId,
    pub source_id: SourceId,
    pub formula: PolymarketFormulaPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PolymarketMarketInfoSnapshot {
    snapshot_id: String,
    source_at_ns: u64,
    fetched_at_ns: u64,
    valid_until_ns: u64,
    fees_enabled: bool,
    fd: Option<PolymarketFeeDescriptor>,
    builder: Option<PolymarketBuilderDescriptor>,
}

impl PolymarketMarketInfoSnapshot {
    pub fn from_json(json: &str) -> Result<Self, PolymarketEconomicsError> {
        serde_json::from_str(json).map_err(|_| PolymarketEconomicsError::InvalidMarketInfo)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct PolymarketFeeDescriptor {
    r: Decimal,
    e: u32,
    to: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct PolymarketBuilderDescriptor {
    profile_id: String,
    maker_rate_bps: Decimal,
    taker_rate_bps: Decimal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NtFeeProjection {
    pub fees_enabled: bool,
    pub rate: Decimal,
    pub exponent: u32,
    pub taker_only: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PolymarketEconomicsError {
    InvalidMarketInfo,
    MissingFeeDescriptor,
    UnsupportedExponent,
    InvalidRate,
    InvalidFillLeg,
    StaleSnapshot,
    NtProjectionDisagreement,
    MissingBuilderDescriptor,
    BuilderProfileMismatch,
    InvalidIdentity,
    InvalidEffect,
}

pub struct PolymarketEconomicsAdapter {
    config: PolymarketEconomicsAdapterConfig,
    snapshot: PolymarketMarketInfoSnapshot,
    platform_plan: PlatformQuotePlan,
    builder_plan: BuilderQuotePlan,
}

enum PlatformQuotePlan {
    FeeFree,
    PriceShaped { rate: Decimal, taker_only: bool },
}

enum BuilderQuotePlan {
    Unavailable,
    Approved {
        profile_id: String,
        maker_rate_bps: Decimal,
        taker_rate_bps: Decimal,
    },
}

impl PolymarketEconomicsAdapter {
    pub fn try_new(
        config: PolymarketEconomicsAdapterConfig,
        snapshot: PolymarketMarketInfoSnapshot,
        nt_projection: Option<NtFeeProjection>,
    ) -> Result<Self, PolymarketEconomicsError> {
        if snapshot.source_at_ns > snapshot.fetched_at_ns
            || snapshot.fetched_at_ns > snapshot.valid_until_ns
        {
            return Err(PolymarketEconomicsError::InvalidMarketInfo);
        }
        let platform_plan = match (snapshot.fees_enabled, snapshot.fd.as_ref()) {
            (false, _) => PlatformQuotePlan::FeeFree,
            (true, None) => return Err(PolymarketEconomicsError::MissingFeeDescriptor),
            (true, Some(descriptor)) if descriptor.e != 1 => {
                return Err(PolymarketEconomicsError::UnsupportedExponent);
            }
            (true, Some(descriptor)) if descriptor.r < Decimal::ZERO => {
                return Err(PolymarketEconomicsError::InvalidRate);
            }
            (true, Some(descriptor)) => PlatformQuotePlan::PriceShaped {
                rate: descriptor.r,
                taker_only: descriptor.to,
            },
        };
        if let Some(projection) = nt_projection {
            let agrees = match &platform_plan {
                PlatformQuotePlan::FeeFree => !projection.fees_enabled,
                PlatformQuotePlan::PriceShaped { rate, taker_only } => {
                    projection.fees_enabled
                        && projection.rate == *rate
                        && projection.exponent == 1
                        && projection.taker_only == *taker_only
                }
            };
            if !agrees {
                return Err(PolymarketEconomicsError::NtProjectionDisagreement);
            }
        }
        let builder_plan = match snapshot.builder.as_ref() {
            None => BuilderQuotePlan::Unavailable,
            Some(builder)
                if builder.maker_rate_bps < Decimal::ZERO
                    || builder.taker_rate_bps < Decimal::ZERO =>
            {
                return Err(PolymarketEconomicsError::InvalidRate);
            }
            Some(builder) => BuilderQuotePlan::Approved {
                profile_id: builder.profile_id.clone(),
                maker_rate_bps: builder.maker_rate_bps,
                taker_rate_bps: builder.taker_rate_bps,
            },
        };
        Ok(Self {
            config,
            snapshot,
            platform_plan,
            builder_plan,
        })
    }

    pub fn quote_components(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<Vec<EstimatedEconomicComponent>, PolymarketEconomicsError> {
        self.validate_snapshot(request)?;
        let mut components = Vec::new();
        match &self.platform_plan {
            PlatformQuotePlan::FeeFree => {}
            PlatformQuotePlan::PriceShaped { rate, taker_only }
                if !*taker_only || request.liquidity_role == LiquidityRoleAssumption::Taker =>
            {
                let amount = -self.platform_fee(request, *rate)?;
                if !amount.is_zero() {
                    components.push(self.component(
                        request,
                        ExecutionKind::ProtocolTrading,
                        self.config.platform_component_id.clone(),
                        self.config.platform_formula_id.clone(),
                        CalculationFactor {
                            factor_id: self.config.platform_rate_factor_id.clone(),
                            value: *rate,
                        },
                        amount,
                    )?);
                }
            }
            PlatformQuotePlan::PriceShaped { .. } => {}
        }

        match (&request.routing.attached_charge, &self.builder_plan) {
            (None, _) => {}
            (Some(_), BuilderQuotePlan::Unavailable) => {
                return Err(PolymarketEconomicsError::MissingBuilderDescriptor);
            }
            (
                Some(attachment),
                BuilderQuotePlan::Approved {
                    profile_id,
                    maker_rate_bps,
                    taker_rate_bps,
                },
            ) => {
                if profile_id != attachment.attachment_id.as_str() {
                    return Err(PolymarketEconomicsError::BuilderProfileMismatch);
                }
                let rate_bps = match request.liquidity_role {
                    LiquidityRoleAssumption::GuaranteedMaker => *maker_rate_bps,
                    LiquidityRoleAssumption::Taker => *taker_rate_bps,
                };
                let amount = -self.builder_fee(request, rate_bps)?;
                if !amount.is_zero() {
                    components.push(self.component(
                        request,
                        ExecutionKind::AttachedRouting,
                        self.config.builder_component_id.clone(),
                        self.config.builder_formula_id.clone(),
                        CalculationFactor {
                            factor_id: self.config.builder_rate_factor_id.clone(),
                            value: rate_bps,
                        },
                        amount,
                    )?);
                }
            }
        }
        Ok(components)
    }

    fn validate_snapshot(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<(), PolymarketEconomicsError> {
        if self.snapshot.source_at_ns > self.snapshot.fetched_at_ns
            || self.snapshot.fetched_at_ns > request.requested_at_ns
            || self.snapshot.valid_until_ns < request.requested_at_ns
        {
            return Err(PolymarketEconomicsError::StaleSnapshot);
        }
        Ok(())
    }

    fn platform_fee(
        &self,
        request: &EconomicQuoteRequest,
        rate: Decimal,
    ) -> Result<Decimal, PolymarketEconomicsError> {
        request
            .planned_fill_legs
            .iter()
            .try_fold(Decimal::ZERO, |total, leg| {
                if leg.price <= Decimal::ZERO
                    || leg.price >= Decimal::ONE
                    || leg.quantity <= Decimal::ZERO
                {
                    return Err(PolymarketEconomicsError::InvalidFillLeg);
                }
                let fee = leg.quantity * rate * leg.price * (Decimal::ONE - leg.price);
                Ok(total
                    + fee.round_dp_with_strategy(
                        self.config.formula.fee_round_decimal_places,
                        self.config.formula.fee_rounding_mode.strategy(),
                    ))
            })
    }

    fn builder_fee(
        &self,
        request: &EconomicQuoteRequest,
        rate_bps: Decimal,
    ) -> Result<Decimal, PolymarketEconomicsError> {
        let notional = request
            .planned_fill_legs
            .iter()
            .try_fold(Decimal::ZERO, |total, leg| {
                if leg.price <= Decimal::ZERO || leg.quantity <= Decimal::ZERO {
                    return Err(PolymarketEconomicsError::InvalidFillLeg);
                }
                Ok(total + leg.price * leg.quantity)
            })?;
        Ok(
            (notional * basis_points_to_fraction(rate_bps)).round_dp_with_strategy(
                self.config.formula.fee_round_decimal_places,
                self.config.formula.fee_rounding_mode.strategy(),
            ),
        )
    }

    fn component(
        &self,
        request: &EconomicQuoteRequest,
        execution_kind: ExecutionKind,
        component_id: crate::economics::EconomicComponentId,
        formula_id: FormulaId,
        calculation_factor: CalculationFactor,
        amount: Decimal,
    ) -> Result<EstimatedEconomicComponent, PolymarketEconomicsError> {
        let snapshot_id = SnapshotId::new(self.snapshot.snapshot_id.clone())
            .map_err(|_| PolymarketEconomicsError::InvalidIdentity)?;
        let point_effect =
            SignedNativeEffect::currency(amount, self.config.collateral_unit.clone())
                .map_err(|_| PolymarketEconomicsError::InvalidEffect)?;
        Ok(EstimatedEconomicComponent {
            component_id,
            class: EconomicClass::Charge,
            kind: EconomicKind::Execution(execution_kind),
            scope: EconomicScope::Decision {
                decision_correlation_id: request.decision_correlation_id.clone(),
            },
            point_effect,
            debit_risk_bound: None,
            admission_treatment: AdmissionTreatment::GuaranteedConditionalOnAction,
            calculation_factors: vec![calculation_factor],
            formula_id,
            source: SourceValidity {
                source_id: self.config.source_id.clone(),
                snapshot_id,
                source_at_ns: self.snapshot.source_at_ns,
                fetched_at_ns: self.snapshot.fetched_at_ns,
                valid_until_ns: self.snapshot.valid_until_ns,
            },
            normalized: None,
        })
    }
}

impl VenueEconomicsAdapter for PolymarketEconomicsAdapter {
    fn quote(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<VenueQuoteEstimate, EconomicsUnavailable> {
        let components = self.quote_components(request).map_err(|_| {
            EconomicsUnavailable::ProviderQuoteUnavailable {
                source_id: self.config.source_id.clone(),
            }
        })?;
        let snapshot_id = SnapshotId::new(self.snapshot.snapshot_id.clone()).map_err(|_| {
            EconomicsUnavailable::ProviderQuoteUnavailable {
                source_id: self.config.source_id.clone(),
            }
        })?;
        Ok(VenueQuoteEstimate {
            authority: SourceValidity {
                source_id: self.config.source_id.clone(),
                snapshot_id,
                source_at_ns: self.snapshot.source_at_ns,
                fetched_at_ns: self.snapshot.fetched_at_ns,
                valid_until_ns: self.snapshot.valid_until_ns,
            },
            dependency_sources: Vec::new(),
            components,
        })
    }
}
