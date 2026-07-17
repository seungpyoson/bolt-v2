use crate::economics::{
    AdmissionTreatment, CalculationFactor, EconomicClass, EconomicKind, EconomicQuoteRequest,
    EconomicScope, EconomicsUnavailable, EstimatedEconomicComponent, ExecutionKind, FormulaId,
    LiquidityRoleAssumption, NativeUnitId, SignedNativeEffect, SnapshotId, SourceId,
    SourceValidity, VenueEconomicsAdapter, VenueQuoteEstimate,
};
use rust_decimal::prelude::ToPrimitive;
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolymarketMarketInfoSnapshot {
    snapshot_id: String,
    source_at_ns: u64,
    fetched_at_ns: u64,
    valid_until_ns: u64,
    fd: Option<PolymarketFeeDescriptor>,
    maker_base_fee: Option<Decimal>,
    taker_base_fee: Option<Decimal>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PolymarketSnapshotMetadata {
    pub snapshot_id: String,
    pub source_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct PolymarketMarketInfoWire {
    #[serde(rename = "gst")]
    _game_start_time: Option<String>,
    r: serde_json::Value,
    t: Vec<PolymarketTokenWire>,
    #[serde(rename = "c")]
    _condition_id: Option<String>,
    mos: Decimal,
    mts: Decimal,
    mbf: Option<Decimal>,
    tbf: Option<Decimal>,
    #[serde(rename = "rfqe")]
    _rfq_enabled: Option<bool>,
    #[serde(rename = "itode")]
    _taker_order_delay_enabled: Option<bool>,
    #[serde(rename = "ibce")]
    _blockaid_check_enabled: bool,
    fd: Option<PolymarketFeeDescriptorWire>,
    #[serde(rename = "oas")]
    _order_age_seconds: Option<u64>,
    #[serde(rename = "ao")]
    _accepting_orders: Option<bool>,
    #[serde(rename = "nr")]
    _negative_risk: Option<bool>,
    #[serde(rename = "cbos")]
    _closed_book_order_support: Option<bool>,
    #[serde(rename = "aot")]
    _accepting_orders_timestamp: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct PolymarketTokenWire {
    t: String,
    o: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct PolymarketFeeDescriptorWire {
    r: Decimal,
    e: Decimal,
    to: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PolymarketFeeDescriptor {
    r: Decimal,
    e: u32,
    to: bool,
}

impl PolymarketMarketInfoSnapshot {
    pub fn from_wire_json(
        metadata: PolymarketSnapshotMetadata,
        json: &str,
    ) -> Result<Self, PolymarketEconomicsError> {
        let wire: PolymarketMarketInfoWire =
            serde_json::from_str(json).map_err(|_| PolymarketEconomicsError::InvalidMarketInfo)?;
        if metadata.snapshot_id.trim().is_empty()
            || wire.r.is_null()
            || wire.t.is_empty()
            || wire
                .t
                .iter()
                .any(|token| token.t.trim().is_empty() || token.o.trim().is_empty())
            || wire.mos <= Decimal::ZERO
            || wire.mts <= Decimal::ZERO
            || wire.mbf.is_some_and(|rate| rate < Decimal::ZERO)
            || wire.tbf.is_some_and(|rate| rate < Decimal::ZERO)
        {
            return Err(PolymarketEconomicsError::InvalidMarketInfo);
        }
        let fd = wire
            .fd
            .map(|descriptor| {
                let exponent = descriptor
                    .e
                    .to_u32()
                    .filter(|exponent| Decimal::from(*exponent) == descriptor.e)
                    .ok_or(PolymarketEconomicsError::UnsupportedExponent)?;
                Ok(PolymarketFeeDescriptor {
                    r: descriptor.r,
                    e: exponent,
                    to: descriptor.to,
                })
            })
            .transpose()?;
        Ok(Self {
            snapshot_id: metadata.snapshot_id,
            source_at_ns: metadata.source_at_ns,
            fetched_at_ns: metadata.fetched_at_ns,
            valid_until_ns: metadata.valid_until_ns,
            fd,
            maker_base_fee: wire.mbf,
            taker_base_fee: wire.tbf,
        })
    }
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
    PriceShaped {
        rate: Decimal,
        exponent: u32,
        taker_only: bool,
    },
}

enum BuilderQuotePlan {
    Unavailable,
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
        let platform_plan = match (
            snapshot.fd.as_ref(),
            snapshot.maker_base_fee,
            snapshot.taker_base_fee,
        ) {
            (None, None, None) => PlatformQuotePlan::FeeFree,
            (Some(descriptor), Some(_), Some(_)) if !matches!(descriptor.e, 1 | 2) => {
                return Err(PolymarketEconomicsError::UnsupportedExponent);
            }
            (Some(descriptor), Some(_), Some(_)) if descriptor.r < Decimal::ZERO => {
                return Err(PolymarketEconomicsError::InvalidRate);
            }
            (Some(descriptor), Some(_), Some(_)) => PlatformQuotePlan::PriceShaped {
                rate: descriptor.r,
                exponent: descriptor.e,
                taker_only: descriptor.to,
            },
            _ => return Err(PolymarketEconomicsError::InvalidMarketInfo),
        };
        if let Some(projection) = nt_projection {
            let agrees = match &platform_plan {
                PlatformQuotePlan::FeeFree => !projection.fees_enabled,
                PlatformQuotePlan::PriceShaped {
                    rate,
                    exponent,
                    taker_only,
                } => {
                    projection.fees_enabled
                        && projection.rate == *rate
                        && projection.exponent == *exponent
                        && projection.taker_only == *taker_only
                }
            };
            if !agrees {
                return Err(PolymarketEconomicsError::NtProjectionDisagreement);
            }
        }
        let builder_plan = BuilderQuotePlan::Unavailable;
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
            PlatformQuotePlan::PriceShaped {
                rate,
                exponent,
                taker_only,
            } if !*taker_only || request.liquidity_role == LiquidityRoleAssumption::Taker => {
                let amount = -self.platform_fee(request, *rate, *exponent)?;
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
        exponent: u32,
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
                let price_shape = leg
                    .price
                    .checked_mul(Decimal::ONE - leg.price)
                    .ok_or(PolymarketEconomicsError::InvalidRate)?;
                let price_shape = match exponent {
                    1 => price_shape,
                    2 => price_shape
                        .checked_mul(price_shape)
                        .ok_or(PolymarketEconomicsError::InvalidRate)?,
                    _ => return Err(PolymarketEconomicsError::UnsupportedExponent),
                };
                let fee = leg
                    .quantity
                    .checked_mul(rate)
                    .and_then(|amount| amount.checked_mul(price_shape))
                    .ok_or(PolymarketEconomicsError::InvalidRate)?;
                Ok(total
                    + fee.round_dp_with_strategy(
                        self.config.formula.fee_round_decimal_places,
                        self.config.formula.fee_rounding_mode.strategy(),
                    ))
            })
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
