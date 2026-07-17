use crate::economics::{
    AdmissionTreatment, CalculationFactor, EconomicClass, EconomicKind, EconomicQuoteRequest,
    EconomicScope, EconomicsUnavailable, EstimatedEconomicComponent, ExecutionKind, FormulaId,
    LiquidityRoleAssumption, NativeUnitId, SignedNativeEffect, SnapshotId, SourceId,
    SourceValidity, VenueEconomicsAdapter, basis_points_to_fraction,
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
    nt_projection: Option<NtFeeProjection>,
}

impl PolymarketEconomicsAdapter {
    pub fn new(
        config: PolymarketEconomicsAdapterConfig,
        snapshot: PolymarketMarketInfoSnapshot,
        nt_projection: Option<NtFeeProjection>,
    ) -> Self {
        Self {
            config,
            snapshot,
            nt_projection,
        }
    }

    pub fn quote_components(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<Vec<EstimatedEconomicComponent>, PolymarketEconomicsError> {
        self.validate_snapshot(request)?;
        let mut components = Vec::new();
        if self.snapshot.fees_enabled {
            let descriptor = self
                .snapshot
                .fd
                .as_ref()
                .ok_or(PolymarketEconomicsError::MissingFeeDescriptor)?;
            if descriptor.e != 1 {
                return Err(PolymarketEconomicsError::UnsupportedExponent);
            }
            if descriptor.r < Decimal::ZERO {
                return Err(PolymarketEconomicsError::InvalidRate);
            }
            let applies =
                !descriptor.to || request.liquidity_role == LiquidityRoleAssumption::Taker;
            if applies {
                let amount = -self.platform_fee(request, descriptor)?;
                if !amount.is_zero() {
                    components.push(self.component(
                        request,
                        self.config.platform_component_id.clone(),
                        self.config.platform_formula_id.clone(),
                        self.config.platform_rate_factor_id.clone(),
                        descriptor.r,
                        amount,
                    )?);
                }
            }
        }

        if let Some(attachment) = &request.routing.attached_charge {
            let builder = self
                .snapshot
                .builder
                .as_ref()
                .ok_or(PolymarketEconomicsError::MissingBuilderDescriptor)?;
            if builder.profile_id != attachment.attachment_id.as_str() {
                return Err(PolymarketEconomicsError::BuilderProfileMismatch);
            }
            let rate_bps = match request.liquidity_role {
                LiquidityRoleAssumption::GuaranteedMaker => builder.maker_rate_bps,
                LiquidityRoleAssumption::Taker => builder.taker_rate_bps,
            };
            if rate_bps < Decimal::ZERO {
                return Err(PolymarketEconomicsError::InvalidRate);
            }
            let amount = -self.builder_fee(request, rate_bps)?;
            if !amount.is_zero() {
                components.push(self.component(
                    request,
                    self.config.builder_component_id.clone(),
                    self.config.builder_formula_id.clone(),
                    self.config.builder_rate_factor_id.clone(),
                    rate_bps,
                    amount,
                )?);
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
        if let Some(projection) = &self.nt_projection {
            let descriptor = self.snapshot.fd.as_ref();
            let agrees = projection.fees_enabled == self.snapshot.fees_enabled
                && (!self.snapshot.fees_enabled
                    || descriptor.is_some_and(|descriptor| {
                        projection.rate == descriptor.r
                            && projection.exponent == descriptor.e
                            && projection.taker_only == descriptor.to
                    }));
            if !agrees {
                return Err(PolymarketEconomicsError::NtProjectionDisagreement);
            }
        }
        Ok(())
    }

    fn platform_fee(
        &self,
        request: &EconomicQuoteRequest,
        descriptor: &PolymarketFeeDescriptor,
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
                let fee = leg.quantity * descriptor.r * leg.price * (Decimal::ONE - leg.price);
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
        component_id: crate::economics::EconomicComponentId,
        formula_id: FormulaId,
        factor_id: FormulaId,
        factor_value: Decimal,
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
            kind: EconomicKind::Execution(ExecutionKind::ProtocolTrading),
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
    ) -> Result<Vec<EstimatedEconomicComponent>, EconomicsUnavailable> {
        self.quote_components(request)
            .map_err(|_| EconomicsUnavailable::ProviderQuoteUnavailable {
                source_id: self.config.source_id.clone(),
            })
    }
}
