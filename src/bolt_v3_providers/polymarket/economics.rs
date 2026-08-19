use std::{collections::BTreeSet, sync::Arc};

use rust_decimal::{Decimal, RoundingStrategy, prelude::ToPrimitive};
use serde::Deserialize;

use crate::{
    bolt_v3_config::{EconomicsRoutingAttachmentPolicy, ExecutionEconomicsConfig},
    bolt_v3_economics_runtime::AuthoritativeVenueEconomicsInput,
    bolt_v3_providers::{
        BuiltProviderEconomicsAdapter, ProviderEconomicsAdapterBuildContext,
        ProviderEconomicsReplayAuthorityBuildContext,
    },
    economics::{
        AdmissionTreatment, CalculationFactor, CurrencyId, EconomicClass, EconomicComponentId,
        EconomicKind, EconomicScope, EconomicsError, EconomicsInstrumentId, EconomicsQuoteRequest,
        EstimatedEffect, ExecutionClientId, ExecutionKind, FormulaId, LiquidityRole,
        PlannedFillNotional, PointEstimate, ProductSurfaceId, RiskBoundAuthority,
        SignedNativeEffect, SnapshotId, SourceIdentity, SourceValidity, VenueEconomicsAdapter,
        VenueEconomicsUnavailable, VenueEdgeBasisEstimate, VenueQuoteEstimate,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeeRoundingMode {
    MidpointNearestEven,
}

impl FeeRoundingMode {
    fn strategy(self) -> RoundingStrategy {
        match self {
            Self::MidpointNearestEven => RoundingStrategy::MidpointNearestEven,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketEconomicsConfig {
    pub instrument_id: EconomicsInstrumentId,
    pub provider_instrument_id: EconomicsInstrumentId,
    pub collateral_currency: CurrencyId,
    pub source: SourceIdentity,
    pub product_surface_id: ProductSurfaceId,
    pub platform_component_id: EconomicComponentId,
    pub platform_formula_id: FormulaId,
    pub platform_rate_factor_id: FormulaId,
    pub fee_round_decimal_places: u32,
    pub fee_rounding_mode: FeeRoundingMode,
    pub edge_basis_resolver_id: FormulaId,
    pub edge_basis_product_metadata_source: SourceIdentity,
    pub edge_basis_policy_version: u64,
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
    _condition_id: ProductSurfaceId,
    token_ids: BTreeSet<EconomicsInstrumentId>,
    platform: PolymarketPlatformPlan,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolymarketReplayEconomicsAuthority {
    provider_instrument_id: String,
    snapshot_id: String,
    source_at_ns: u64,
    fetched_at_ns: u64,
    valid_until_ns: u64,
    market_info_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PolymarketAuthoritativeEconomics {
    snapshot: PolymarketMarketInfoSnapshot,
    provider_instrument_id: EconomicsInstrumentId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolymarketPlatformPlan {
    FeeDescriptorUnknown,
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
    #[serde(default, rename = "mbf")]
    _maker_builder_fee_metadata: Option<Decimal>,
    #[serde(default, rename = "tbf")]
    _taker_builder_fee_metadata: Option<Decimal>,
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
    FeeDescriptorUnknown,
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
            Self::FeeDescriptorUnknown => {
                f.write_str("Polymarket fee descriptor is absent and therefore unknown")
            }
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
        let platform = match wire.fd {
            None => PolymarketPlatformPlan::FeeDescriptorUnknown,
            Some(descriptor) if descriptor.r >= Decimal::ZERO => {
                let exponent = descriptor
                    .e
                    .to_u32()
                    .ok_or(PolymarketEconomicsError::UnsupportedExponent)?;
                if Decimal::from(exponent) != descriptor.e || exponent != 1 {
                    return Err(PolymarketEconomicsError::UnsupportedExponent);
                }
                PolymarketPlatformPlan::PriceShaped {
                    rate: descriptor.r,
                    exponent,
                    taker_only: descriptor.to,
                }
            }
            Some(_) => return Err(PolymarketEconomicsError::InvalidMarketInfo),
        };
        Ok(Self {
            metadata,
            _condition_id: condition_id,
            token_ids,
            platform,
        })
    }
}

pub fn authoritative_economics_input(
    execution_client_id: impl Into<String>,
    instrument_id: impl Into<String>,
    product_surface_id: impl Into<String>,
    provider_instrument_id: impl Into<String>,
    snapshot: PolymarketMarketInfoSnapshot,
) -> Result<AuthoritativeVenueEconomicsInput, PolymarketEconomicsError> {
    let execution_client_id = execution_client_id.into();
    let instrument_id = EconomicsInstrumentId::try_new(instrument_id.into())?;
    let product_surface_id = ProductSurfaceId::try_new(product_surface_id.into())?;
    let provider_instrument_id = EconomicsInstrumentId::try_new(provider_instrument_id.into())?;
    ExecutionClientId::try_new(execution_client_id.clone())?;
    if !snapshot.token_ids.contains(&provider_instrument_id) {
        return Err(PolymarketEconomicsError::InvalidRequestScope);
    }
    Ok(AuthoritativeVenueEconomicsInput::from_provider_authority(
        execution_client_id,
        instrument_id.as_str(),
        product_surface_id.as_str(),
        super::KEY,
        Arc::new(PolymarketAuthoritativeEconomics {
            snapshot,
            provider_instrument_id,
        }),
    ))
}

pub(crate) fn build_replay_economics_authority(
    context: ProviderEconomicsReplayAuthorityBuildContext<'_>,
) -> Result<AuthoritativeVenueEconomicsInput, String> {
    let replay: PolymarketReplayEconomicsAuthority = context
        .authority
        .clone()
        .try_into()
        .map_err(|error| format!("invalid Polymarket replay economics authority: {error}"))?;
    let snapshot = PolymarketMarketInfoSnapshot::from_json(
        PolymarketSnapshotMetadata {
            snapshot_id: SnapshotId::try_new(replay.snapshot_id)
                .map_err(|error| error.to_string())?,
            source_at_ns: replay.source_at_ns,
            fetched_at_ns: replay.fetched_at_ns,
            valid_until_ns: replay.valid_until_ns,
        },
        &replay.market_info_json,
    )
    .map_err(|error| error.to_string())?;
    authoritative_economics_input(
        context.execution_client_id,
        context.instrument_id,
        context.product_surface_id,
        replay.provider_instrument_id,
        snapshot,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn build_execution_economics_adapter(
    context: ProviderEconomicsAdapterBuildContext<'_>,
) -> Result<BuiltProviderEconomicsAdapter, String> {
    let authority = context
        .authority
        .downcast_ref::<PolymarketAuthoritativeEconomics>()
        .ok_or_else(|| "Polymarket economics authority has the wrong snapshot type".to_string())?;
    let execution = context
        .execution
        .clone()
        .try_into::<super::PolymarketExecutionConfig>()
        .map_err(|error| error.to_string())?;
    let config = adapter_config_from_toml(
        context.config,
        context.instrument_id,
        context.product_surface_id,
        &authority.provider_instrument_id,
    )?;
    PolymarketEconomicsAdapter::try_new(config, authority.snapshot.clone())
        .map(|adapter| BuiltProviderEconomicsAdapter {
            account_id: execution.account_id.to_string(),
            adapter: Arc::new(adapter),
        })
        .map_err(|error| error.to_string())
}

fn adapter_config_from_toml(
    config: &ExecutionEconomicsConfig,
    instrument_id: &str,
    product_surface_id: &str,
    provider_instrument_id: &EconomicsInstrumentId,
) -> Result<PolymarketEconomicsConfig, String> {
    validate_execution_economics_config(config)?;
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
    let id_error = |error: EconomicsError| error.to_string();
    Ok(PolymarketEconomicsConfig {
        instrument_id: EconomicsInstrumentId::try_new(instrument_id).map_err(id_error)?,
        provider_instrument_id: provider_instrument_id.clone(),
        collateral_currency: CurrencyId::try_new(collateral.currency.clone()).map_err(id_error)?,
        source: SourceIdentity::try_new(config.sources["schedule"].clone()).map_err(id_error)?,
        product_surface_id: ProductSurfaceId::try_new(product_surface_id).map_err(id_error)?,
        platform_component_id: EconomicComponentId::try_new(platform.component_id.clone())
            .map_err(id_error)?,
        platform_formula_id: FormulaId::try_new(platform.formula_id.clone()).map_err(id_error)?,
        platform_rate_factor_id: FormulaId::try_new(platform.rate_factor_id.clone())
            .map_err(id_error)?,
        fee_round_decimal_places,
        fee_rounding_mode: FeeRoundingMode::MidpointNearestEven,
        edge_basis_resolver_id: FormulaId::try_new(edge_basis.resolver_id.clone())
            .map_err(id_error)?,
        edge_basis_product_metadata_source: SourceIdentity::try_new(
            edge_basis.product_metadata_source.clone(),
        )
        .map_err(id_error)?,
        edge_basis_policy_version: edge_basis.policy_version,
    })
}

pub(crate) fn validate_execution_economics_config(
    config: &ExecutionEconomicsConfig,
) -> Result<(), String> {
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
    if config.formula["sub_fee_quantum_behavior"] != "round_to_nearest_even" {
        return Err(
            "Polymarket sub-fee quantum behavior must be round_to_nearest_even".to_string(),
        );
    }
    config.formula["fee_round_decimal_places"]
        .parse::<u32>()
        .map_err(|_| "Polymarket fee_round_decimal_places is invalid".to_string())?;
    if config.formula["fee_rounding_mode"] != "midpoint_nearest_even" {
        return Err(
            "Polymarket fee_rounding_mode must be midpoint_nearest_even when sub-fee quantum behavior is round_to_nearest_even"
                .to_string(),
        );
    }
    let id_error = |error: EconomicsError| error.to_string();
    SourceIdentity::try_new(config.sources["schedule"].clone()).map_err(id_error)?;
    let platform = &config.quote_components["platform"];
    EconomicComponentId::try_new(platform.component_id.clone()).map_err(id_error)?;
    FormulaId::try_new(platform.formula_id.clone()).map_err(id_error)?;
    FormulaId::try_new(platform.rate_factor_id.clone()).map_err(id_error)?;
    CurrencyId::try_new(config.assets["collateral"].currency.clone()).map_err(id_error)?;
    for edge_basis in config.edge_basis.values() {
        FormulaId::try_new(edge_basis.resolver_id.clone()).map_err(id_error)?;
        SourceIdentity::try_new(edge_basis.product_metadata_source.clone()).map_err(id_error)?;
    }
    Ok(())
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
        if !snapshot.token_ids.contains(&config.provider_instrument_id) {
            return Err(PolymarketEconomicsError::InvalidRequestScope);
        }
        if matches!(
            snapshot.platform,
            PolymarketPlatformPlan::PriceShaped {
                exponent,
                rate,
                ..
            } if exponent != 1 || rate < Decimal::ZERO
        ) {
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
        if self.snapshot.platform == PolymarketPlatformPlan::FeeDescriptorUnknown {
            return Err(PolymarketEconomicsError::FeeDescriptorUnknown);
        }
        if let PolymarketPlatformPlan::PriceShaped {
            rate, taker_only, ..
        } = self.snapshot.platform
            && (!taker_only || request.liquidity_role == LiquidityRole::Taker)
        {
            let platform_fee = self.platform_fee(request, rate)?;
            let debit_risk_bound = self.platform_fee_bound(request, rate)?;
            if rate.is_zero() || !platform_fee.is_zero() || !debit_risk_bound.is_zero() {
                components.push(self.effect(
                    request,
                    EffectPlan {
                        component_id: self.config.platform_component_id.clone(),
                        formula_id: self.config.platform_formula_id.clone(),
                        rate_factor_id: self.config.platform_rate_factor_id.clone(),
                        rate,
                        kind: ExecutionKind::ProtocolTrading,
                        fee: platform_fee,
                        debit_risk_bound: (!rate.is_zero()).then_some(debit_risk_bound),
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
            || request.instrument_id != self.config.instrument_id
            || request.routing.attached_charge.is_some()
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
                    || leg.price > Decimal::ONE
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

    fn platform_fee_bound(
        &self,
        request: &EconomicsQuoteRequest,
        rate: Decimal,
    ) -> Result<Decimal, PolymarketEconomicsError> {
        request
            .planned_fill_legs
            .iter()
            .try_fold(Decimal::ZERO, |total, leg| {
                if leg.quantity <= Decimal::ZERO {
                    return Err(PolymarketEconomicsError::InvalidFillLeg);
                }
                let fee = leg
                    .quantity
                    .checked_mul(rate)
                    .and_then(|value| value.checked_mul(Decimal::new(25, 2)))
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
        let rate_factor = CalculationFactor {
            factor_id: plan.rate_factor_id.clone(),
            value: plan.rate,
        };
        let (point_estimate, calculation_factors) = if plan.fee.is_zero() {
            let evaluated_formula = CalculationFactor {
                factor_id: plan.formula_id.clone(),
                value: Decimal::ZERO,
            };
            (
                PointEstimate::ProvenZero {
                    factor_id: evaluated_formula.factor_id.clone(),
                },
                vec![rate_factor, evaluated_formula],
            )
        } else {
            (
                PointEstimate::NonZero(SignedNativeEffect::currency(
                    -plan.fee,
                    self.config.collateral_currency.clone(),
                )?),
                vec![rate_factor],
            )
        };
        Ok(EstimatedEffect {
            component_id: plan.component_id,
            class: EconomicClass::Charge,
            kind: EconomicKind::Execution(plan.kind),
            scope: EconomicScope::Decision {
                decision_correlation_id: request.decision_correlation_id.clone(),
            },
            point_estimate,
            debit_risk_bound: plan
                .debit_risk_bound
                .map(|bound| {
                    SignedNativeEffect::currency(-bound, self.config.collateral_currency.clone())
                })
                .transpose()?,
            admission_treatment: if plan.debit_risk_bound.is_some() {
                AdmissionTreatment::RiskBound {
                    authority: RiskBoundAuthority::VenueRateCapWithPriceStress,
                }
            } else {
                AdmissionTreatment::GuaranteedConditionalOnAction
            },
            calculation_factors,
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
            | PolymarketEconomicsError::FeeDescriptorUnknown
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
    debit_risk_bound: Option<Decimal>,
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
            instrument_id: id("token-yes.POLYMARKET", EconomicsInstrumentId::try_new),
            provider_instrument_id: id("token-yes", EconomicsInstrumentId::try_new),
            collateral_currency: id("pUSD", CurrencyId::try_new),
            source: id("clob-market-info", SourceIdentity::try_new),
            product_surface_id: id("condition", ProductSurfaceId::try_new),
            platform_component_id: id("platform", EconomicComponentId::try_new),
            platform_formula_id: id("platform-v2", FormulaId::try_new),
            platform_rate_factor_id: id("platform-rate", FormulaId::try_new),
            fee_round_decimal_places: 5,
            fee_rounding_mode: FeeRoundingMode::MidpointNearestEven,
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
        }
    }

    fn request(role: LiquidityRole, routed: bool) -> EconomicsQuoteRequest {
        request_for("token-yes.POLYMARKET", "condition", role, routed)
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
    fn taker_market_metadata_prices_only_the_authoritative_platform_fee() {
        let adapter = adapter(include_str!(
            "../../../tests/fixtures/economics/polymarket/fee_enabled.json"
        ))
        .expect("fee-enabled fixture should construct");
        let request = request(LiquidityRole::Taker, false);
        let estimate = adapter
            .quote(&request)
            .expect("supported quote should resolve");
        assert_eq!(estimate.components.len(), 1);
        assert!(estimate.components.iter().all(|component| matches!(
            component.admission_treatment,
            AdmissionTreatment::RiskBound { .. }
        )));
        let quote = validate_and_aggregate_quote(&request, estimate, &[])
            .expect("native pUSD quote should aggregate");
        let [platform_fee] = quote.components() else {
            panic!("the platform fee must be the sole component");
        };
        assert_eq!(
            platform_fee
                .point_valuation()
                .expect("the platform fee must carry a point valuation")
                .normalized_amount,
            Decimal::new(-222, 3)
        );
        assert_eq!(quote.core_total(), Decimal::new(-225, 3));
    }

    #[test]
    fn taker_platform_fee_accepts_the_binary_max_price_with_a_conservative_bound() {
        let adapter = adapter(include_str!(
            "../../../tests/fixtures/economics/polymarket/fee_enabled.json"
        ))
        .expect("fee-enabled fixture should construct");
        let mut request = request(LiquidityRole::Taker, false);
        request.planned_fill_legs = vec![PlannedFillLeg {
            price: Decimal::ONE,
            quantity: Decimal::new(3, 0),
        }];

        let estimate = adapter
            .quote(&request)
            .expect("the venue's valid binary max price should quote");
        assert_eq!(estimate.components.len(), 1);
        assert!(matches!(
            estimate.components[0].point_estimate,
            PointEstimate::ProvenZero { .. }
        ));
        assert!(
            estimate.components[0]
                .debit_risk_bound
                .as_ref()
                .is_some_and(|bound| bound.amount() < Decimal::ZERO),
            "the zero point estimate must retain a conservative debit bound"
        );
        validate_and_aggregate_quote(&request, estimate, &[])
            .expect("the boundary-price estimate must satisfy the shared economics contract");
    }

    #[test]
    fn maker_skips_taker_only_platform_even_when_market_metadata_has_builder_fields() {
        let adapter = adapter(include_str!(
            "../../../tests/fixtures/economics/polymarket/fee_enabled.json"
        ))
        .expect("fee-enabled fixture should construct");
        let request = request(LiquidityRole::GuaranteedMaker, false);
        let estimate = adapter
            .quote(&request)
            .expect("supported quote should resolve");
        assert!(estimate.components.is_empty());
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
    }

    #[test]
    fn absent_fee_descriptor_always_fails_closed() {
        let snapshot = PolymarketMarketInfoSnapshot::from_json(
            metadata(),
            include_str!("../../../tests/fixtures/economics/polymarket/fee_free.json"),
        )
        .expect("descriptor-absent fixture should parse as typed unknown");
        let request = request(LiquidityRole::Taker, false);
        let fail_closed = PolymarketEconomicsAdapter::try_new(config(), snapshot.clone())
            .expect("typed-unknown adapter should construct");
        assert_eq!(
            fail_closed.quote(&request),
            Err(PolymarketEconomicsError::FeeDescriptorUnknown)
        );
    }

    #[test]
    fn explicit_zero_fee_descriptor_is_provider_sourced_proven_zero() {
        let snapshot = PolymarketMarketInfoSnapshot::from_json(
            metadata(),
            include_str!("../../../tests/fixtures/economics/polymarket/explicit_zero_fee.json"),
        )
        .expect("explicit-zero descriptor fixture should parse");
        let request = request(LiquidityRole::Taker, false);
        let estimate = PolymarketEconomicsAdapter::try_new(config(), snapshot)
            .expect("explicit-zero adapter should construct")
            .quote(&request)
            .expect("provider-sourced explicit zero should admit");
        let [component] = estimate.components.as_slice() else {
            panic!("explicit provider zero must emit one audit component");
        };
        assert!(matches!(
            component.point_estimate,
            PointEstimate::ProvenZero { .. }
        ));
        assert_eq!(
            component.admission_treatment,
            AdmissionTreatment::GuaranteedConditionalOnAction
        );
        assert_eq!(component.source, estimate.authority);
    }

    #[test]
    fn point_fee_rounding_matches_pinned_nt_calculate_commission_at_boundaries() {
        use nautilus_model::enums::LiquiditySide;
        use nautilus_polymarket::execution::parse::compute_commission;

        let adapter = adapter(include_str!(
            "../../../tests/fixtures/economics/polymarket/fee_enabled.json"
        ))
        .expect("fee-enabled fixture should construct");
        for price in [
            Decimal::new(365, 4),
            Decimal::new(728, 4),
            Decimal::new(9635, 4),
        ] {
            let mut request = request(LiquidityRole::Taker, false);
            request.planned_fill_legs = vec![PlannedFillLeg {
                price,
                quantity: Decimal::ONE,
            }];
            let estimate = adapter
                .quote(&request)
                .expect("rounding-boundary request should quote");
            let [component] = estimate.components.as_slice() else {
                panic!("fee-bearing boundary fixture should emit one component");
            };
            let bolt_point = match &component.point_estimate {
                PointEstimate::NonZero(effect) => -effect.amount(),
                PointEstimate::ProvenZero { .. } => Decimal::ZERO,
            };
            let nt_point = Decimal::from_str_exact(&format!(
                "{:.5}",
                compute_commission(
                    Decimal::new(3, 2),
                    1.0,
                    Decimal::ONE,
                    price,
                    LiquiditySide::Taker,
                )
            ))
            .expect("NT commission should format as a decimal");
            let superseded_truncation = Decimal::new(3, 2)
                .checked_mul(price)
                .and_then(|value| value.checked_mul(Decimal::ONE - price))
                .expect("rounding fixture arithmetic should fit")
                .round_dp_with_strategy(5, RoundingStrategy::ToZero);
            let reserved_debit = -component
                .debit_risk_bound
                .as_ref()
                .expect("fee point must retain its conservative debit bound")
                .amount();

            assert_eq!(bolt_point, nt_point, "price {price}");
            assert_ne!(bolt_point, superseded_truncation, "price {price}");
            assert!(reserved_debit >= bolt_point, "price {price}");
        }
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
        captured_config.instrument_id = id(
            "43187333641922996188398060383389814287787647811837308994701068387397271207198.POLYMARKET",
            EconomicsInstrumentId::try_new,
        );
        captured_config.provider_instrument_id = id(
            "43187333641922996188398060383389814287787647811837308994701068387397271207198",
            EconomicsInstrumentId::try_new,
        );
        captured_config.product_surface_id = id(product_surface_id, ProductSurfaceId::try_new);
        let adapter = PolymarketEconomicsAdapter::try_new(captured_config, snapshot)
            .expect("captured fee-bearing market info should construct");
        let request = request_for(
            "43187333641922996188398060383389814287787647811837308994701068387397271207198.POLYMARKET",
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

        let [platform_fee] = quote.components() else {
            panic!("captured schedule should produce one platform fee component");
        };
        assert_eq!(
            platform_fee
                .point_valuation()
                .expect("captured point fee should be valued")
                .normalized_amount,
            Decimal::new(-518, 3)
        );
        assert_eq!(quote.core_total(), Decimal::new(-525, 3));
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
        let metadata_only = adapter(&partial_descriptor.to_string())
            .expect("builder metadata without a platform fee descriptor should remain typed");
        assert_eq!(
            metadata_only.quote(&request(LiquidityRole::Taker, false)),
            Err(PolymarketEconomicsError::FeeDescriptorUnknown)
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
