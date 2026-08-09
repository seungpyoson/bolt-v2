use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive};
use serde::Deserialize;

use crate::economics::{
    AdmissionTreatment, CalculationFactor, CurrencyId, EconomicClass, EconomicComponentId,
    EconomicKind, EconomicScope, EconomicsError, EconomicsQuoteRequest, EstimatedEffect,
    ExecutionKind, FormulaId, LiquidityRole, PointEstimate, SignedNativeEffect, SnapshotId,
    SourceIdentity, SourceValidity, VenueQuoteEstimate,
};

const BASIS_POINTS_PER_UNIT: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketEconomicsConfig {
    pub collateral_currency: CurrencyId,
    pub source: SourceIdentity,
    pub platform_component_id: EconomicComponentId,
    pub platform_formula_id: FormulaId,
    pub platform_rate_factor_id: FormulaId,
    pub builder_component_id: EconomicComponentId,
    pub builder_formula_id: FormulaId,
    pub builder_rate_factor_id: FormulaId,
    pub fee_round_decimal_places: u32,
    pub fee_rounding_mode: FeeRoundingMode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketSnapshotMetadata {
    pub snapshot_id: SnapshotId,
    pub source_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketMarketInfoSnapshot {
    metadata: PolymarketSnapshotMetadata,
    platform: PolymarketPlatformPlan,
    builder_maker_fee_bps: Decimal,
    builder_taker_fee_bps: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolymarketPlatformPlan {
    FeeFree,
    PriceShaped {
        rate: Decimal,
        exponent: u32,
        taker_only: bool,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolymarketMarketInfoWire {
    #[serde(default, rename = "gst")]
    _game_start_time: Option<String>,
    r: serde_json::Value,
    t: Vec<PolymarketTokenWire>,
    #[serde(default, rename = "c")]
    _condition_id: Option<String>,
    mos: Decimal,
    mts: Decimal,
    mbf: Option<Decimal>,
    tbf: Option<Decimal>,
    #[serde(default, rename = "rfqe")]
    _rfq_enabled: Option<bool>,
    #[serde(default, rename = "itode")]
    _taker_order_delay_enabled: Option<bool>,
    #[serde(default, rename = "ibce")]
    _blockaid_check_enabled: Option<bool>,
    fd: Option<PolymarketFeeDescriptorWire>,
    #[serde(default, rename = "oas")]
    _order_age_seconds: Option<u64>,
    #[serde(default, rename = "ao")]
    _accepting_orders: Option<bool>,
    #[serde(default, rename = "nr")]
    _negative_risk: Option<bool>,
    #[serde(default, rename = "cbos")]
    _closed_book_order_support: Option<bool>,
    #[serde(default, rename = "aot")]
    _accepting_orders_timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolymarketTokenWire {
    t: String,
    o: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolymarketFeeDescriptorWire {
    r: Decimal,
    e: Decimal,
    to: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolymarketEconomicsError {
    InvalidMarketInfo,
    UnsupportedExponent,
    InvalidRate,
    InvalidFillLeg,
    ArithmeticOverflow,
    InvalidEffect(EconomicsError),
}

impl std::fmt::Display for PolymarketEconomicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidMarketInfo => f.write_str("Polymarket market-info is invalid"),
            Self::UnsupportedExponent => {
                f.write_str("Polymarket fee exponent is not fixture-supported")
            }
            Self::InvalidRate => f.write_str("Polymarket fee rate is invalid"),
            Self::InvalidFillLeg => f.write_str("Polymarket fill leg is invalid"),
            Self::ArithmeticOverflow => f.write_str("Polymarket fee arithmetic overflowed"),
            Self::InvalidEffect(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for PolymarketEconomicsError {}

impl From<EconomicsError> for PolymarketEconomicsError {
    fn from(value: EconomicsError) -> Self {
        Self::InvalidEffect(value)
    }
}

impl PolymarketMarketInfoSnapshot {
    pub fn from_json(
        metadata: PolymarketSnapshotMetadata,
        json: &str,
    ) -> Result<Self, PolymarketEconomicsError> {
        let wire: PolymarketMarketInfoWire =
            serde_json::from_str(json).map_err(|_| PolymarketEconomicsError::InvalidMarketInfo)?;
        if metadata.source_at_ns > metadata.fetched_at_ns
            || metadata.fetched_at_ns > metadata.valid_until_ns
            || wire.r.is_null()
            || wire.t.is_empty()
            || wire
                .t
                .iter()
                .any(|token| token.t.trim().is_empty() || token.o.trim().is_empty())
            || wire.mos <= Decimal::ZERO
            || wire.mts <= Decimal::ZERO
        {
            return Err(PolymarketEconomicsError::InvalidMarketInfo);
        }
        let (platform, builder_maker_fee_bps, builder_taker_fee_bps) =
            match (wire.fd, wire.mbf, wire.tbf) {
                (None, None, None) => (
                    PolymarketPlatformPlan::FeeFree,
                    Decimal::ZERO,
                    Decimal::ZERO,
                ),
                (Some(descriptor), Some(maker), Some(taker))
                    if descriptor.r >= Decimal::ZERO
                        && maker >= Decimal::ZERO
                        && taker >= Decimal::ZERO =>
                {
                    let exponent = descriptor
                        .e
                        .to_u32()
                        .ok_or(PolymarketEconomicsError::UnsupportedExponent)?;
                    if Decimal::from(exponent) != descriptor.e || exponent != 1 {
                        return Err(PolymarketEconomicsError::UnsupportedExponent);
                    }
                    (
                        PolymarketPlatformPlan::PriceShaped {
                            rate: descriptor.r,
                            exponent,
                            taker_only: descriptor.to,
                        },
                        maker,
                        taker,
                    )
                }
                _ => return Err(PolymarketEconomicsError::InvalidMarketInfo),
            };
        Ok(Self {
            metadata,
            platform,
            builder_maker_fee_bps,
            builder_taker_fee_bps,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketEconomicsAdapter {
    config: PolymarketEconomicsConfig,
    snapshot: PolymarketMarketInfoSnapshot,
}

impl PolymarketEconomicsAdapter {
    pub fn try_new(
        config: PolymarketEconomicsConfig,
        snapshot: PolymarketMarketInfoSnapshot,
    ) -> Result<Self, PolymarketEconomicsError> {
        if matches!(
            snapshot.platform,
            PolymarketPlatformPlan::PriceShaped {
                exponent,
                rate,
                ..
            } if exponent != 1 || rate < Decimal::ZERO
        ) || snapshot.builder_maker_fee_bps < Decimal::ZERO
            || snapshot.builder_taker_fee_bps < Decimal::ZERO
        {
            return Err(PolymarketEconomicsError::InvalidRate);
        }
        Ok(Self { config, snapshot })
    }

    pub fn quote(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<VenueQuoteEstimate, PolymarketEconomicsError> {
        request.validate()?;
        let authority = self.authority();
        let mut components = Vec::new();
        if let PolymarketPlatformPlan::PriceShaped {
            rate, taker_only, ..
        } = self.snapshot.platform
            && (!taker_only || request.liquidity_role == LiquidityRole::Taker)
        {
            let platform_fee = self.platform_fee(request, rate)?;
            if !platform_fee.is_zero() {
                components.push(self.effect(
                    request,
                    EffectPlan {
                        component_id: self.config.platform_component_id.clone(),
                        formula_id: self.config.platform_formula_id.clone(),
                        rate_factor_id: self.config.platform_rate_factor_id.clone(),
                        rate,
                        kind: ExecutionKind::ProtocolTrading,
                        fee: platform_fee,
                    },
                )?);
            }
        }
        if request.routing.attached_charge.is_some() {
            let builder_rate_bps = match request.liquidity_role {
                LiquidityRole::GuaranteedMaker => self.snapshot.builder_maker_fee_bps,
                LiquidityRole::Taker => self.snapshot.builder_taker_fee_bps,
            };
            let builder_fee = self.builder_fee(request, builder_rate_bps)?;
            if !builder_fee.is_zero() {
                components.push(self.effect(
                    request,
                    EffectPlan {
                        component_id: self.config.builder_component_id.clone(),
                        formula_id: self.config.builder_formula_id.clone(),
                        rate_factor_id: self.config.builder_rate_factor_id.clone(),
                        rate: builder_rate_bps,
                        kind: ExecutionKind::AttachedRoutingCharge,
                        fee: builder_fee,
                    },
                )?);
            }
        }
        Ok(VenueQuoteEstimate {
            authority,
            dependency_sources: Vec::new(),
            components,
        })
    }

    fn authority(&self) -> SourceValidity {
        SourceValidity {
            source: self.config.source.clone(),
            snapshot_id: self.snapshot.metadata.snapshot_id.clone(),
            source_at_ns: self.snapshot.metadata.source_at_ns,
            fetched_at_ns: self.snapshot.metadata.fetched_at_ns,
            valid_until_ns: self.snapshot.metadata.valid_until_ns,
        }
    }

    fn platform_fee(
        &self,
        request: &EconomicsQuoteRequest,
        rate: Decimal,
    ) -> Result<Decimal, PolymarketEconomicsError> {
        if !matches!(
            self.snapshot.platform,
            PolymarketPlatformPlan::PriceShaped { exponent: 1, .. }
        ) {
            return Err(PolymarketEconomicsError::UnsupportedExponent);
        }
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
                let fee = leg
                    .quantity
                    .checked_mul(rate)
                    .and_then(|value| value.checked_mul(leg.price))
                    .and_then(|value| value.checked_mul(Decimal::ONE - leg.price))
                    .ok_or(PolymarketEconomicsError::ArithmeticOverflow)?
                    .round_dp_with_strategy(
                        self.config.fee_round_decimal_places,
                        self.config.fee_rounding_mode.strategy(),
                    );
                total
                    .checked_add(fee)
                    .ok_or(PolymarketEconomicsError::ArithmeticOverflow)
            })
    }

    fn builder_fee(
        &self,
        request: &EconomicsQuoteRequest,
        rate_bps: Decimal,
    ) -> Result<Decimal, PolymarketEconomicsError> {
        request
            .planned_fill_legs
            .iter()
            .try_fold(Decimal::ZERO, |total, leg| {
                if leg.price <= Decimal::ZERO || leg.quantity <= Decimal::ZERO {
                    return Err(PolymarketEconomicsError::InvalidFillLeg);
                }
                let fee = leg
                    .price
                    .checked_mul(leg.quantity)
                    .and_then(|value| value.checked_mul(rate_bps))
                    .and_then(|value| value.checked_div(Decimal::from(BASIS_POINTS_PER_UNIT)))
                    .ok_or(PolymarketEconomicsError::ArithmeticOverflow)?
                    .round_dp_with_strategy(
                        self.config.fee_round_decimal_places,
                        self.config.fee_rounding_mode.strategy(),
                    );
                total
                    .checked_add(fee)
                    .ok_or(PolymarketEconomicsError::ArithmeticOverflow)
            })
    }

    fn effect(
        &self,
        request: &EconomicsQuoteRequest,
        plan: EffectPlan,
    ) -> Result<EstimatedEffect, PolymarketEconomicsError> {
        Ok(EstimatedEffect {
            component_id: plan.component_id,
            class: EconomicClass::Charge,
            kind: EconomicKind::Execution(plan.kind),
            scope: EconomicScope::Decision {
                decision_correlation_id: request.decision_correlation_id.clone(),
            },
            point_estimate: PointEstimate::NonZero(SignedNativeEffect::currency(
                -plan.fee,
                self.config.collateral_currency.clone(),
            )?),
            debit_risk_bound: None,
            admission_treatment: AdmissionTreatment::GuaranteedConditionalOnAction,
            calculation_factors: vec![CalculationFactor {
                factor_id: plan.rate_factor_id,
                value: plan.rate,
            }],
            formula_id: plan.formula_id,
            source: self.authority(),
        })
    }
}

struct EffectPlan {
    component_id: EconomicComponentId,
    formula_id: FormulaId,
    rate_factor_id: FormulaId,
    rate: Decimal,
    kind: ExecutionKind,
    fee: Decimal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economics::{
        AccountId, DecisionCorrelationId, EconomicsInstrumentId, EdgeBasisPolicyId,
        ExecutionClientId, LifecyclePath, OrderSide, PlannedFillLeg, ProductSurfaceId,
        ReportingPolicyId, RoutingAttachmentId, RoutingContext, validate_and_aggregate_quote,
    };

    fn id<T>(value: &str, constructor: impl FnOnce(String) -> Result<T, EconomicsError>) -> T {
        constructor(value.to_owned()).expect("fixture identifier should be canonical")
    }

    fn config() -> PolymarketEconomicsConfig {
        PolymarketEconomicsConfig {
            collateral_currency: id("pUSD", CurrencyId::try_new),
            source: id("clob-market-info", SourceIdentity::try_new),
            platform_component_id: id("platform", EconomicComponentId::try_new),
            platform_formula_id: id("platform-v2", FormulaId::try_new),
            platform_rate_factor_id: id("platform-rate", FormulaId::try_new),
            builder_component_id: id("builder", EconomicComponentId::try_new),
            builder_formula_id: id("builder-v2", FormulaId::try_new),
            builder_rate_factor_id: id("builder-rate", FormulaId::try_new),
            fee_round_decimal_places: 5,
            fee_rounding_mode: FeeRoundingMode::MidpointAwayFromZero,
        }
    }

    fn metadata() -> PolymarketSnapshotMetadata {
        PolymarketSnapshotMetadata {
            snapshot_id: id("market-info-1", SnapshotId::try_new),
            source_at_ns: 900,
            fetched_at_ns: 950,
            valid_until_ns: 1_100,
        }
    }

    fn request(role: LiquidityRole, routed: bool) -> EconomicsQuoteRequest {
        EconomicsQuoteRequest {
            execution_client_id: id("execution", ExecutionClientId::try_new),
            account_id: id("account", AccountId::try_new),
            instrument_id: id("condition-token", EconomicsInstrumentId::try_new),
            product_surface_id: id("binary", ProductSurfaceId::try_new),
            order_side: OrderSide::Buy,
            liquidity_role: role,
            planned_fill_legs: vec![
                PlannedFillLeg {
                    price: Decimal::new(4, 1),
                    quantity: Decimal::new(10, 0),
                },
                PlannedFillLeg {
                    price: Decimal::new(5, 1),
                    quantity: Decimal::new(20, 0),
                },
            ],
            routing: RoutingContext {
                attached_charge: routed.then(|| id("builder-code", RoutingAttachmentId::try_new)),
            },
            position: None,
            lifecycle_path: LifecyclePath::HoldToRedemption,
            reporting_policy_id: id("reporting", ReportingPolicyId::try_new),
            reporting_currency: id("pUSD", CurrencyId::try_new),
            edge_basis_policy_id: id("basis", EdgeBasisPolicyId::try_new),
            requested_at_ns: 1_000,
            decision_correlation_id: id("decision", DecisionCorrelationId::try_new),
        }
    }

    fn adapter(fixture: &str) -> Result<PolymarketEconomicsAdapter, PolymarketEconomicsError> {
        PolymarketEconomicsAdapter::try_new(
            config(),
            PolymarketMarketInfoSnapshot::from_json(metadata(), fixture)?,
        )
    }

    #[test]
    fn taker_platform_and_builder_fees_are_separate_and_rounded_per_level() {
        let adapter = adapter(include_str!(
            "../../../tests/fixtures/economics/polymarket/fee_enabled.json"
        ))
        .expect("fee-enabled fixture should construct");
        let request = request(LiquidityRole::Taker, true);
        let estimate = adapter
            .quote(&request)
            .expect("supported quote should resolve");
        assert_eq!(estimate.components.len(), 2);
        let quote = validate_and_aggregate_quote(&request, estimate, &[])
            .expect("native pUSD quote should aggregate");

        assert_eq!(quote.core_total(), Decimal::new(-362, 3));
    }

    #[test]
    fn maker_skips_taker_only_platform_but_pays_attached_builder_fee() {
        let adapter = adapter(include_str!(
            "../../../tests/fixtures/economics/polymarket/fee_enabled.json"
        ))
        .expect("fee-enabled fixture should construct");
        let request = request(LiquidityRole::GuaranteedMaker, true);
        let estimate = adapter
            .quote(&request)
            .expect("supported quote should resolve");
        assert_eq!(estimate.components.len(), 1);
        assert_eq!(
            estimate.components[0].kind,
            EconomicKind::Execution(ExecutionKind::AttachedRoutingCharge)
        );
    }

    #[test]
    fn authoritative_fee_free_schedule_produces_no_component() {
        let adapter = adapter(include_str!(
            "../../../tests/fixtures/economics/polymarket/fee_free.json"
        ))
        .expect("fee-free fixture should construct");
        let request = request(LiquidityRole::Taker, false);
        assert!(
            adapter
                .quote(&request)
                .expect("fee-free quote should resolve")
                .components
                .is_empty()
        );
    }

    #[test]
    fn captured_fee_bearing_market_info_drives_decimal_formula_directly() {
        let adapter = adapter(include_str!(
            "../../../tests/fixtures/economics/polymarket/captured_fee_bearing.json"
        ))
        .expect("captured fee-bearing market info should construct");
        let request = request(LiquidityRole::Taker, false);
        let quote = validate_and_aggregate_quote(
            &request,
            adapter
                .quote(&request)
                .expect("captured schedule should quote"),
            &[],
        )
        .expect("captured schedule should aggregate");

        assert_eq!(quote.core_total(), Decimal::new(-518, 3));
    }

    #[test]
    fn unsupported_missing_and_stale_authority_fail_closed() {
        assert_eq!(
            adapter(include_str!(
                "../../../tests/fixtures/economics/polymarket/unsupported_exponent.json"
            )),
            Err(PolymarketEconomicsError::UnsupportedExponent)
        );
        let mut partial_descriptor: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/economics/polymarket/fee_enabled.json"
        ))
        .expect("fixture should be JSON");
        partial_descriptor
            .as_object_mut()
            .expect("fixture should be an object")
            .remove("fd");
        assert_eq!(
            adapter(&partial_descriptor.to_string()),
            Err(PolymarketEconomicsError::InvalidMarketInfo)
        );
        let mut stale_metadata = metadata();
        stale_metadata.valid_until_ns = 999;
        let snapshot = PolymarketMarketInfoSnapshot::from_json(
            stale_metadata,
            include_str!("../../../tests/fixtures/economics/polymarket/fee_enabled.json"),
        )
        .expect("well-formed stale snapshot should parse");
        let estimate = PolymarketEconomicsAdapter::try_new(config(), snapshot)
            .expect("timeline-valid adapter should construct")
            .quote(&request(LiquidityRole::Taker, false))
            .expect("adapter should preserve source validity");
        assert!(matches!(
            validate_and_aggregate_quote(&request(LiquidityRole::Taker, false), estimate, &[]),
            Err(EconomicsError::StaleSource { .. })
        ));
    }
}
