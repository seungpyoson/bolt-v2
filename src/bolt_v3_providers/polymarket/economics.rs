use std::{collections::BTreeSet, sync::Arc};

use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive};
use serde::Deserialize;

use crate::{
    bolt_v3_config::{EconomicsRoutingAttachmentPolicy, ExecutionEconomicsConfig},
    bolt_v3_economics_runtime::AuthoritativeVenueEconomicsInput,
    bolt_v3_providers::{BuiltProviderEconomicsAdapter, ProviderEconomicsAdapterBuildContext},
    economics::{
        AdmissionTreatment, CalculationFactor, CurrencyId, EconomicClass, EconomicComponentId,
        EconomicKind, EconomicScope, EconomicsError, EconomicsInstrumentId, EconomicsQuoteRequest,
        EstimatedEffect, ExecutionClientId, ExecutionKind, FormulaId, LiquidityRole,
        PlannedFillNotional, PointEstimate, ProductSurfaceId, RoutingAttachmentId,
        SignedNativeEffect, SnapshotId, SourceIdentity, SourceValidity, VenueEconomicsAdapter,
        VenueEconomicsUnavailable, VenueEdgeBasisEstimate, VenueQuoteEstimate,
    },
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
    pub product_surface_id: ProductSurfaceId,
    pub platform_component_id: EconomicComponentId,
    pub platform_formula_id: FormulaId,
    pub platform_rate_factor_id: FormulaId,
    pub routing: Option<PolymarketRoutingEconomicsConfig>,
    pub fee_round_decimal_places: u32,
    pub fee_rounding_mode: FeeRoundingMode,
    pub edge_basis_resolver_id: FormulaId,
    pub edge_basis_product_metadata_source: SourceIdentity,
    pub edge_basis_policy_version: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketRoutingEconomicsConfig {
    pub component_id: EconomicComponentId,
    pub formula_id: FormulaId,
    pub rate_factor_id: FormulaId,
    pub attachment_id: RoutingAttachmentId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketSnapshotMetadata {
    pub snapshot_id: SnapshotId,
    pub source_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
    pub builder_attachment_id: Option<RoutingAttachmentId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketMarketInfoSnapshot {
    metadata: PolymarketSnapshotMetadata,
    _condition_id: ProductSurfaceId,
    token_ids: BTreeSet<EconomicsInstrumentId>,
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
    condition_id: Option<String>,
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
    InvalidRequestScope,
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
            Self::InvalidRequestScope => {
                f.write_str("Polymarket quote request does not match its market authority")
            }
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
        let condition_id = wire
            .condition_id
            .and_then(|value| ProductSurfaceId::try_new(value).ok())
            .ok_or(PolymarketEconomicsError::InvalidMarketInfo)?;
        let token_ids = wire
            .t
            .iter()
            .map(|token| EconomicsInstrumentId::try_new(token.t.clone()))
            .collect::<Result<BTreeSet<_>, _>>()
            .map_err(|_| PolymarketEconomicsError::InvalidMarketInfo)?;
        if token_ids.len() != wire.t.len() {
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
            _condition_id: condition_id,
            token_ids,
            platform,
            builder_maker_fee_bps,
            builder_taker_fee_bps,
        })
    }
}

pub fn authoritative_economics_input(
    execution_client_id: impl Into<String>,
    instrument_id: impl Into<String>,
    product_surface_id: impl Into<String>,
    snapshot: PolymarketMarketInfoSnapshot,
) -> Result<AuthoritativeVenueEconomicsInput, PolymarketEconomicsError> {
    let execution_client_id = execution_client_id.into();
    let instrument_id = EconomicsInstrumentId::try_new(instrument_id.into())?;
    let product_surface_id = ProductSurfaceId::try_new(product_surface_id.into())?;
    ExecutionClientId::try_new(execution_client_id.clone())?;
    if !snapshot.token_ids.contains(&instrument_id) {
        return Err(PolymarketEconomicsError::InvalidRequestScope);
    }
    Ok(AuthoritativeVenueEconomicsInput::from_provider_authority(
        execution_client_id,
        instrument_id.as_str(),
        product_surface_id.as_str(),
        super::KEY,
        Arc::new(snapshot),
    ))
}

pub(crate) fn build_execution_economics_adapter(
    context: ProviderEconomicsAdapterBuildContext<'_>,
) -> Result<BuiltProviderEconomicsAdapter, String> {
    let snapshot = context
        .authority
        .downcast_ref::<PolymarketMarketInfoSnapshot>()
        .ok_or_else(|| "Polymarket economics authority has the wrong snapshot type".to_string())?;
    let execution = context
        .execution
        .clone()
        .try_into::<super::PolymarketExecutionConfig>()
        .map_err(|error| error.to_string())?;
    let config = adapter_config_from_toml(context.config, context.product_surface_id)?;
    PolymarketEconomicsAdapter::try_new(config, snapshot.clone())
        .map(|adapter| BuiltProviderEconomicsAdapter {
            account_id: execution.account_id.to_string(),
            adapter: Arc::new(adapter),
        })
        .map_err(|error| error.to_string())
}

fn adapter_config_from_toml(
    config: &ExecutionEconomicsConfig,
    product_surface_id: &str,
) -> Result<PolymarketEconomicsConfig, String> {
    if config.routing_attachment_policy != EconomicsRoutingAttachmentPolicy::Forbidden {
        return Err("Polymarket Slice 1 requires routing attachments to be forbidden".to_string());
    }
    if config
        .sources
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        != ["schedule"]
        || config
            .formula
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != [
                "fee_round_decimal_places",
                "fee_rounding_mode",
                "sub_fee_quantum_behavior",
            ]
        || config
            .quote_components
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != ["platform"]
        || config.assets.keys().map(String::as_str).collect::<Vec<_>>() != ["collateral"]
        || config.carry.is_some()
        || !config.carry_surfaces.is_empty()
    {
        return Err("Polymarket economics contains unsupported authority keys".to_string());
    }
    if config.formula["sub_fee_quantum_behavior"] != "round_to_zero" {
        return Err("Polymarket sub-fee quantum behavior must be round_to_zero".to_string());
    }
    let platform = &config.quote_components["platform"];
    let collateral = &config.assets["collateral"];
    let policy_id = config
        .product_surface_policies
        .get(product_surface_id)
        .ok_or_else(|| {
            format!("Polymarket product surface `{product_surface_id}` has no edge-basis policy")
        })?;
    let edge_basis = config
        .edge_basis
        .get(policy_id)
        .ok_or_else(|| format!("Polymarket edge-basis policy `{policy_id}` is not configured"))?;
    let fee_round_decimal_places = config.formula["fee_round_decimal_places"]
        .parse::<u32>()
        .map_err(|_| "Polymarket fee_round_decimal_places is invalid".to_string())?;
    let fee_rounding_mode = match config.formula["fee_rounding_mode"].as_str() {
        "midpoint_away_from_zero" => FeeRoundingMode::MidpointAwayFromZero,
        "to_zero" => FeeRoundingMode::ToZero,
        _ => return Err("Polymarket fee_rounding_mode is unsupported".to_string()),
    };
    let id_error = |error: EconomicsError| error.to_string();
    Ok(PolymarketEconomicsConfig {
        collateral_currency: CurrencyId::try_new(collateral.currency.clone()).map_err(id_error)?,
        source: SourceIdentity::try_new(config.sources["schedule"].clone()).map_err(id_error)?,
        product_surface_id: ProductSurfaceId::try_new(product_surface_id).map_err(id_error)?,
        platform_component_id: EconomicComponentId::try_new(platform.component_id.clone())
            .map_err(id_error)?,
        platform_formula_id: FormulaId::try_new(platform.formula_id.clone()).map_err(id_error)?,
        platform_rate_factor_id: FormulaId::try_new(platform.rate_factor_id.clone())
            .map_err(id_error)?,
        routing: None,
        fee_round_decimal_places,
        fee_rounding_mode,
        edge_basis_resolver_id: FormulaId::try_new(edge_basis.resolver_id.clone())
            .map_err(id_error)?,
        edge_basis_product_metadata_source: SourceIdentity::try_new(
            edge_basis.product_metadata_source.clone(),
        )
        .map_err(id_error)?,
        edge_basis_policy_version: edge_basis.policy_version,
    })
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
        if config
            .routing
            .as_ref()
            .map(|routing| &routing.attachment_id)
            != snapshot.metadata.builder_attachment_id.as_ref()
        {
            return Err(PolymarketEconomicsError::InvalidRequestScope);
        }
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
        self.validate_request_authority(request)?;
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
            let routing = self
                .config
                .routing
                .as_ref()
                .ok_or(PolymarketEconomicsError::InvalidRequestScope)?;
            let builder_rate_bps = match request.liquidity_role {
                LiquidityRole::GuaranteedMaker => self.snapshot.builder_maker_fee_bps,
                LiquidityRole::Taker => self.snapshot.builder_taker_fee_bps,
            };
            let builder_fee = self.builder_fee(request, builder_rate_bps)?;
            if !builder_fee.is_zero() {
                components.push(self.effect(
                    request,
                    EffectPlan {
                        component_id: routing.component_id.clone(),
                        formula_id: routing.formula_id.clone(),
                        rate_factor_id: routing.rate_factor_id.clone(),
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

    fn validate_request_authority(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<(), PolymarketEconomicsError> {
        if request.product_surface_id != self.config.product_surface_id
            || !self.snapshot.token_ids.contains(&request.instrument_id)
            || match (
                request.routing.attached_charge.as_ref(),
                self.config.routing.as_ref(),
            ) {
                (None, _) => false,
                (Some(attached), Some(routing)) => attached != &routing.attachment_id,
                (Some(_), None) => true,
            }
        {
            return Err(PolymarketEconomicsError::InvalidRequestScope);
        }
        Ok(())
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

impl VenueEconomicsAdapter for PolymarketEconomicsAdapter {
    fn provider_key(&self) -> &str {
        super::KEY
    }

    fn resolve_edge_basis(
        &self,
        request: &EconomicsQuoteRequest,
        planned_fill_notional: PlannedFillNotional,
    ) -> Result<VenueEdgeBasisEstimate, VenueEconomicsUnavailable> {
        self.validate_request_authority(request)
            .map_err(|_| VenueEconomicsUnavailable::RequestScopeMismatch)?;
        Ok(VenueEdgeBasisEstimate {
            resolver_id: self.config.edge_basis_resolver_id.clone(),
            product_metadata_source: self.config.edge_basis_product_metadata_source.clone(),
            policy_version: self.config.edge_basis_policy_version,
            normalized_amount: crate::economics::EdgeBasisAmount::try_new(
                planned_fill_notional.amount(),
            )?,
            source_snapshot_ids: vec![self.snapshot.metadata.snapshot_id.clone()],
            valid_until_ns: self.snapshot.metadata.valid_until_ns,
        })
    }

    fn quote(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<VenueQuoteEstimate, VenueEconomicsUnavailable> {
        PolymarketEconomicsAdapter::quote(self, request).map_err(|error| match error {
            PolymarketEconomicsError::UnsupportedExponent => {
                VenueEconomicsUnavailable::UnsupportedProductEconomics
            }
            PolymarketEconomicsError::InvalidRequestScope => {
                VenueEconomicsUnavailable::RequestScopeMismatch
            }
            PolymarketEconomicsError::InvalidEffect(error) => error.into(),
            PolymarketEconomicsError::InvalidMarketInfo
            | PolymarketEconomicsError::InvalidRate
            | PolymarketEconomicsError::InvalidFillLeg
            | PolymarketEconomicsError::ArithmeticOverflow => {
                VenueEconomicsUnavailable::InvalidAuthoritativeSnapshot
            }
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
            product_surface_id: id("condition", ProductSurfaceId::try_new),
            platform_component_id: id("platform", EconomicComponentId::try_new),
            platform_formula_id: id("platform-v2", FormulaId::try_new),
            platform_rate_factor_id: id("platform-rate", FormulaId::try_new),
            routing: Some(PolymarketRoutingEconomicsConfig {
                component_id: id("builder", EconomicComponentId::try_new),
                formula_id: id("builder-v2", FormulaId::try_new),
                rate_factor_id: id("builder-rate", FormulaId::try_new),
                attachment_id: id("builder-code", RoutingAttachmentId::try_new),
            }),
            fee_round_decimal_places: 5,
            fee_rounding_mode: FeeRoundingMode::MidpointAwayFromZero,
            edge_basis_resolver_id: id("product-metadata", FormulaId::try_new),
            edge_basis_product_metadata_source: id(
                "polymarket-market-info",
                SourceIdentity::try_new,
            ),
            edge_basis_policy_version: 1,
        }
    }

    fn metadata() -> PolymarketSnapshotMetadata {
        PolymarketSnapshotMetadata {
            snapshot_id: id("market-info-1", SnapshotId::try_new),
            source_at_ns: 900,
            fetched_at_ns: 950,
            valid_until_ns: 1_100,
            builder_attachment_id: Some(id("builder-code", RoutingAttachmentId::try_new)),
        }
    }

    fn request(role: LiquidityRole, routed: bool) -> EconomicsQuoteRequest {
        request_for("token-yes", "condition", role, routed)
    }

    fn request_for(
        instrument_id: &str,
        product_surface_id: &str,
        role: LiquidityRole,
        routed: bool,
    ) -> EconomicsQuoteRequest {
        EconomicsQuoteRequest {
            execution_client_id: id("execution", ExecutionClientId::try_new),
            account_id: id("account", AccountId::try_new),
            instrument_id: id(instrument_id, EconomicsInstrumentId::try_new),
            product_surface_id: id(product_surface_id, ProductSurfaceId::try_new),
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
    fn quote_rejects_foreign_market_token_surface_and_builder_attachment() {
        let adapter = adapter(include_str!(
            "../../../tests/fixtures/economics/polymarket/fee_enabled.json"
        ))
        .expect("fee-enabled fixture should construct");

        let mut foreign_token = request(LiquidityRole::Taker, false);
        foreign_token.instrument_id = id("foreign-token", EconomicsInstrumentId::try_new);
        assert_eq!(
            adapter.quote(&foreign_token),
            Err(PolymarketEconomicsError::InvalidRequestScope)
        );

        let mut foreign_surface = request(LiquidityRole::Taker, false);
        foreign_surface.product_surface_id = id("foreign-condition", ProductSurfaceId::try_new);
        assert_eq!(
            adapter.quote(&foreign_surface),
            Err(PolymarketEconomicsError::InvalidRequestScope)
        );

        let mut foreign_builder = request(LiquidityRole::Taker, true);
        foreign_builder.routing.attached_charge =
            Some(id("foreign-builder-code", RoutingAttachmentId::try_new));
        assert_eq!(
            adapter.quote(&foreign_builder),
            Err(PolymarketEconomicsError::InvalidRequestScope)
        );

        let mut foreign_builder_metadata = metadata();
        foreign_builder_metadata.builder_attachment_id =
            Some(id("foreign-builder-code", RoutingAttachmentId::try_new));
        let snapshot = PolymarketMarketInfoSnapshot::from_json(
            foreign_builder_metadata,
            include_str!("../../../tests/fixtures/economics/polymarket/fee_enabled.json"),
        )
        .expect("foreign-builder snapshot should remain structurally valid");
        assert_eq!(
            PolymarketEconomicsAdapter::try_new(config(), snapshot),
            Err(PolymarketEconomicsError::InvalidRequestScope)
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
        let product_surface_id =
            "0x0e7b7cc2649466ce6dfed9cf49611630fe986b31fba84ec01107e0a50f1534bb";
        let snapshot = PolymarketMarketInfoSnapshot::from_json(
            metadata(),
            include_str!("../../../tests/fixtures/economics/polymarket/captured_fee_bearing.json"),
        )
        .expect("captured fee-bearing market info should parse");
        let mut captured_config = config();
        captured_config.product_surface_id = id(product_surface_id, ProductSurfaceId::try_new);
        let adapter = PolymarketEconomicsAdapter::try_new(captured_config, snapshot)
            .expect("captured fee-bearing market info should construct");
        let request = request_for(
            "43187333641922996188398060383389814287787647811837308994701068387397271207198",
            product_surface_id,
            LiquidityRole::Taker,
            false,
        );
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
