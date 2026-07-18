use crate::{
    bolt_v3_economics_runtime::{
        AuthoritativeEdgeBasis, ProviderEconomicsAuthority, ProviderEconomicsAuthoritySnapshot,
    },
    economics::{
        AdmissionTreatment, CalculationFactor, CarryKind, EconomicClass, EconomicKind,
        EconomicQuoteRequest, EconomicScope, EconomicsUnavailable, EstimatedEconomicComponent,
        ExecutionKind, FormulaId, LiquidityRoleAssumption, NativeUnitId, PositionSide,
        RiskBoundAuthority, SignedNativeEffect, SnapshotId, SourceId, SourceValidity,
        VenueEconomicsAdapter, VenueQuoteEstimate, basis_points_to_fraction,
    },
};
use anyhow::Context;
use async_trait::async_trait;
use nautilus_core::consts::NAUTILUS_USER_AGENT;
use nautilus_model::{
    identifiers::Venue,
    instruments::{Instrument, InstrumentAny},
};
use nautilus_network::http::{HttpClient, USER_AGENT};
use rust_decimal::Decimal;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::str::FromStr;
use std::{collections::HashMap, sync::Arc};
use zeroize::Zeroizing;

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

impl HyperliquidEconomicsAdapterConfig {
    pub fn from_execution_config(
        economics: &crate::bolt_v3_economics_config::ExecutionEconomicsConfig,
    ) -> Result<Self, HyperliquidEconomicsError> {
        let protocol = economics
            .quote_components
            .get("protocol")
            .ok_or(HyperliquidEconomicsError::InvalidIdentity)?;
        let builder = economics
            .quote_components
            .get("builder")
            .ok_or(HyperliquidEconomicsError::InvalidIdentity)?;
        let settlement = economics
            .assets
            .get("settlement")
            .ok_or(HyperliquidEconomicsError::InvalidIdentity)?;
        let decimal = |key: &str| {
            economics
                .formula
                .get(key)
                .and_then(|value| Decimal::from_str(value).ok())
                .ok_or(HyperliquidEconomicsError::InvalidIdentity)
        };
        let carry = economics
            .carry
            .as_ref()
            .map(|carry| {
                Ok(HyperliquidCarryPolicy {
                    component_id: crate::economics::EconomicComponentId::new(
                        carry.component_id.clone(),
                    )
                    .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
                    formula_id: FormulaId::new(carry.formula_id.clone())
                        .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
                    point_rate_factor_id: FormulaId::new(carry.point_rate_factor_id.clone())
                        .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
                    bound_rate_factor_id: FormulaId::new(carry.bound_rate_factor_id.clone())
                        .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
                    risk_policy_id: FormulaId::new(carry.risk_policy_id.clone())
                        .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
                    stress_fixture_id: FormulaId::new(carry.stress_fixture_id.clone())
                        .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
                    funding_interval_ns: carry
                        .funding_interval_secs
                        .checked_mul(crate::bolt_v3_numeric::MILLIS_PER_SECOND_U64)
                        .and_then(|value| {
                            value.checked_mul(crate::bolt_v3_numeric::NANOS_PER_MILLI_U64)
                        })
                        .ok_or(HyperliquidEconomicsError::InvalidIdentity)?,
                    venue_rate_cap_fraction: basis_points_to_fraction(
                        Decimal::from_str(&carry.funding_venue_rate_cap_bps_per_hour)
                            .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
                    ),
                    standard_price_stress_multiplier: Decimal::from_str(
                        &carry.funding_standard_price_stress_multiplier,
                    )
                    .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
                })
            })
            .transpose()?;
        Ok(Self {
            settlement_unit: NativeUnitId::new(settlement.native_unit.clone())
                .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
            protocol_component_id: crate::economics::EconomicComponentId::new(
                protocol.component_id.clone(),
            )
            .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
            protocol_formula_id: FormulaId::new(protocol.formula_id.clone())
                .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
            protocol_rate_factor_id: FormulaId::new(protocol.rate_factor_id.clone())
                .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
            builder_component_id: crate::economics::EconomicComponentId::new(
                builder.component_id.clone(),
            )
            .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
            builder_formula_id: FormulaId::new(builder.formula_id.clone())
                .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
            builder_rate_factor_id: FormulaId::new(builder.rate_factor_id.clone())
                .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
            source_id: SourceId::new(
                economics
                    .sources
                    .get("account_fees")
                    .ok_or(HyperliquidEconomicsError::InvalidIdentity)?
                    .clone(),
            )
            .map_err(|_| HyperliquidEconomicsError::InvalidIdentity)?,
            formula: HyperliquidFormulaPolicy {
                stable_pair_scale: decimal("stable_pair_scale")?,
                growth_mode_scale: decimal("growth_mode_scale")?,
                hip3_scale_threshold: decimal("hip3_scale_threshold")?,
                hip3_below_threshold_base: decimal("hip3_below_threshold_base")?,
                hip3_at_or_above_threshold_multiplier: decimal(
                    "hip3_at_or_above_threshold_multiplier",
                )?,
                hip3_at_or_above_deployer_share: decimal("hip3_at_or_above_deployer_share")?,
            },
            carry,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyperliquidCarryPolicy {
    pub component_id: crate::economics::EconomicComponentId,
    pub formula_id: FormulaId,
    pub point_rate_factor_id: FormulaId,
    pub bound_rate_factor_id: FormulaId,
    pub risk_policy_id: FormulaId,
    pub stress_fixture_id: FormulaId,
    pub funding_interval_ns: u64,
    pub venue_rate_cap_fraction: Decimal,
    pub standard_price_stress_multiplier: Decimal,
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
    trial_escrow: Decimal,
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
    fee_trial_escrow: Decimal,
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(transparent)]
#[allow(dead_code)]
struct RequiredNullable<T>(Option<T>);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(dead_code)]
struct HyperliquidPerpMetaWire {
    universe: Vec<HyperliquidPerpProductWire>,
    margin_tables: Vec<(u32, HyperliquidMarginTableWire)>,
    collateral_token: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(dead_code)]
struct HyperliquidPerpProductWire {
    name: String,
    sz_decimals: u32,
    max_leverage: u32,
    margin_table_id: u32,
    is_delisted: Option<bool>,
    margin_mode: Option<String>,
    only_isolated: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(dead_code)]
struct HyperliquidMarginTableWire {
    description: String,
    margin_tiers: Vec<HyperliquidMarginTierWire>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(dead_code)]
struct HyperliquidMarginTierWire {
    lower_bound: Decimal,
    max_leverage: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
#[allow(dead_code)]
struct HyperliquidAssetContextWire {
    funding: Decimal,
    open_interest: Decimal,
    prev_day_px: Decimal,
    day_ntl_vlm: Decimal,
    premium: RequiredNullable<Decimal>,
    oracle_px: Decimal,
    mark_px: Decimal,
    mid_px: RequiredNullable<Decimal>,
    impact_pxs: RequiredNullable<Vec<Decimal>>,
    day_base_vlm: Decimal,
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
            trial_escrow: wire.fee_trial_escrow,
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
    let staking_scale = Decimal::ONE - wire.active_staking_discount.discount;
    let nonnegative_rates = schedule.cross >= Decimal::ZERO
        && schedule.add >= Decimal::ZERO
        && schedule.spot_cross >= Decimal::ZERO
        && schedule.spot_add >= Decimal::ZERO
        && wire.user_cross_rate >= Decimal::ZERO
        && wire.user_spot_cross_rate >= Decimal::ZERO;
    let perp_taker_rates =
        std::iter::once(schedule.cross).chain(schedule.tiers.vip.iter().map(|tier| tier.cross));
    let perp_maker_rates = std::iter::once(schedule.add)
        .chain(schedule.tiers.vip.iter().map(|tier| tier.add))
        .chain(schedule.tiers.mm.iter().map(|tier| tier.add));
    let spot_taker_rates = std::iter::once(schedule.spot_cross)
        .chain(schedule.tiers.vip.iter().map(|tier| tier.spot_cross));
    let spot_maker_rates = std::iter::once(schedule.spot_add)
        .chain(schedule.tiers.vip.iter().map(|tier| tier.spot_add));
    let effective_rates_consistent =
        effective_rate_matches_schedule(wire.user_cross_rate, perp_taker_rates, staking_scale)
            && effective_rate_matches_schedule(wire.user_add_rate, perp_maker_rates, staking_scale)
            && effective_rate_matches_schedule(
                wire.user_spot_cross_rate,
                spot_taker_rates,
                staking_scale,
            )
            && effective_rate_matches_schedule(
                wire.user_spot_add_rate,
                spot_maker_rates,
                staking_scale,
            );
    let tier_rates_valid = schedule.tiers.vip.iter().all(|tier| {
        tier.ntl_cutoff >= Decimal::ZERO
            && tier.cross >= Decimal::ZERO
            && tier.add >= Decimal::ZERO
            && tier.spot_cross >= Decimal::ZERO
            && tier.spot_add >= Decimal::ZERO
    }) && schedule
        .tiers
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
            .any(|tier| tier == &wire.active_staking_discount);
    let staking_link_valid = wire
        .staking_link
        .as_ref()
        .is_none_or(|link| !link.r#type.trim().is_empty() && !link.staking_user.trim().is_empty());
    nonnegative_rates
        && effective_rates_consistent
        && tier_rates_valid
        && staking_valid
        && staking_link_valid
        && wire.fee_trial_escrow >= Decimal::ZERO
        && unit_interval(schedule.referral_discount)
        && unit_interval(wire.active_referral_discount)
        && wire.active_referral_discount <= schedule.referral_discount
        && wire.active_staking_discount.bps_of_max_supply >= Decimal::ZERO
        && unit_interval(wire.active_staking_discount.discount)
}

fn effective_rate_matches_schedule(
    effective: Decimal,
    schedule_rates: impl Iterator<Item = Decimal>,
    staking_scale: Decimal,
) -> bool {
    schedule_rates.into_iter().any(|scheduled| {
        let expected = if scheduled < Decimal::ZERO {
            scheduled
        } else {
            scheduled * staking_scale
        };
        effective == expected
    })
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
    carry_oracle_price: Option<Decimal>,
    carry_point_rate_per_ns: Option<Decimal>,
    carry_debit_rate_bound_per_ns: Option<Decimal>,
}

impl HyperliquidProductEconomicsSnapshot {
    pub fn from_json(json: &str) -> Result<Self, HyperliquidEconomicsError> {
        serde_json::from_str(json).map_err(|_| HyperliquidEconomicsError::InvalidProductMetadata)
    }

    pub fn from_perp_meta_wire(
        metadata: HyperliquidSnapshotMetadata,
        json: &[u8],
        raw_symbol: &str,
        carry: &HyperliquidCarryPolicy,
    ) -> Result<Self, HyperliquidEconomicsError> {
        let (meta, contexts): (HyperliquidPerpMetaWire, Vec<HyperliquidAssetContextWire>) =
            serde_json::from_slice(json)
                .map_err(|_| HyperliquidEconomicsError::InvalidProductMetadata)?;
        if metadata.snapshot_id.trim().is_empty()
            || metadata.source_at_ns > metadata.fetched_at_ns
            || metadata.fetched_at_ns > metadata.valid_until_ns
            || meta.universe.is_empty()
            || meta.margin_tables.is_empty()
            || meta.universe.len() != contexts.len()
            || carry.funding_interval_ns == 0
            || carry.venue_rate_cap_fraction <= Decimal::ZERO
        {
            return Err(HyperliquidEconomicsError::InvalidProductMetadata);
        }
        let (product, context) = meta
            .universe
            .iter()
            .zip(contexts.iter())
            .find(|(product, _)| product.name == raw_symbol)
            .ok_or(HyperliquidEconomicsError::InvalidProductMetadata)?;
        if product.is_delisted.is_some()
            || product.margin_mode.is_some()
            || product.only_isolated.is_some()
            || product.max_leverage == 0
            || !meta
                .margin_tables
                .iter()
                .any(|(table_id, _)| *table_id == product.margin_table_id)
            || context.oracle_px <= Decimal::ZERO
            || context.mark_px <= Decimal::ZERO
        {
            return Err(HyperliquidEconomicsError::InvalidProductMetadata);
        }
        let point_rate = context
            .funding
            .checked_div(Decimal::from(carry.funding_interval_ns))
            .ok_or(HyperliquidEconomicsError::InvalidProductMetadata)?;
        let bound_rate = carry
            .venue_rate_cap_fraction
            .checked_div(Decimal::from(carry.funding_interval_ns))
            .ok_or(HyperliquidEconomicsError::InvalidProductMetadata)?;
        Ok(Self {
            snapshot_id: metadata.snapshot_id,
            source_at_ns: metadata.source_at_ns,
            fetched_at_ns: metadata.fetched_at_ns,
            valid_until_ns: metadata.valid_until_ns,
            product_kind: HyperliquidProductKind::Perp,
            base_unit: None,
            quote_unit: None,
            stable_pair: false,
            aligned_quote_or_collateral: false,
            hip3: false,
            deployer_scale: Decimal::ZERO,
            growth_mode: false,
            builder_profile_id: None,
            builder_rate_bps: None,
            builder_approved_max_bps: None,
            spot_dust_authority_complete: false,
            carry_oracle_price: Some(context.oracle_px),
            carry_point_rate_per_ns: Some(point_rate),
            carry_debit_rate_bound_per_ns: Some(bound_rate),
        })
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

pub struct HyperliquidEconomicsAuthority {
    execution_client_id: String,
    account_id: String,
    account_address: Zeroizing<String>,
    venue: Venue,
    economics: crate::bolt_v3_economics_config::ExecutionEconomicsConfig,
    adapter_config: HyperliquidEconomicsAdapterConfig,
    product_surface_id: String,
    base_url_http: String,
    http_timeout_secs: u64,
    http_client: HttpClient,
}

impl HyperliquidEconomicsAuthority {
    pub fn try_new(
        execution_client_id: &str,
        venue: Venue,
        execution: super::HyperliquidExecutionConfig,
        secrets: &super::ResolvedBoltV3HyperliquidSecrets,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            execution.product_surfaces == vec![super::HyperliquidProductSurface::StandardPerps],
            "Hyperliquid quote authority currently requires exactly the configured standard_perps surface"
        );
        let adapter_config =
            HyperliquidEconomicsAdapterConfig::from_execution_config(&execution.economics)
                .map_err(|error| anyhow::anyhow!("invalid economics adapter config: {error:?}"))?;
        anyhow::ensure!(
            execution.economics.product_surface_policies.len() == 1,
            "Hyperliquid economics requires exactly one configured product surface"
        );
        let product_surface_id = execution
            .economics
            .product_surface_policies
            .iter()
            .next()
            .context("Hyperliquid economics product surface is missing")?
            .0
            .clone();
        let http_client = HttpClient::new(
            HashMap::from([
                (USER_AGENT.to_string(), NAUTILUS_USER_AGENT.to_string()),
                ("content-type".to_string(), "application/json".to_string()),
            ]),
            Vec::new(),
            Vec::new(),
            None,
            Some(execution.http_timeout_secs),
            execution.proxy_url.clone(),
        )
        .context("could not build Hyperliquid economics HTTP client")?;
        Ok(Self {
            execution_client_id: execution_client_id.to_string(),
            account_id: execution.account_id.to_string(),
            account_address: Zeroizing::new(secrets.account_address.as_str().to_string()),
            venue,
            economics: execution.economics,
            adapter_config,
            product_surface_id,
            base_url_http: execution.base_url_http,
            http_timeout_secs: execution.http_timeout_secs,
            http_client,
        })
    }

    async fn post_info(&self, body: serde_json::Value) -> anyhow::Result<Vec<u8>> {
        let response = self
            .http_client
            .post(
                self.base_url_http.clone(),
                None,
                None,
                Some(serde_json::to_vec(&body)?),
                Some(self.http_timeout_secs),
                None,
            )
            .await
            .context("Hyperliquid economics info request failed")?;
        anyhow::ensure!(
            response.status.is_success(),
            "Hyperliquid economics info request returned HTTP status {}",
            response.status.as_u16()
        );
        Ok(response.body.to_vec())
    }
}

#[async_trait(?Send)]
impl ProviderEconomicsAuthority for HyperliquidEconomicsAuthority {
    fn execution_client_id(&self) -> &str {
        &self.execution_client_id
    }

    fn provider_key(&self) -> &str {
        self.venue.as_str()
    }

    fn venue(&self) -> Venue {
        self.venue
    }

    fn economics_config(&self) -> &crate::bolt_v3_economics_config::ExecutionEconomicsConfig {
        &self.economics
    }

    async fn refresh(
        &self,
        instrument: InstrumentAny,
        refreshed_at_ns: u64,
    ) -> anyhow::Result<ProviderEconomicsAuthoritySnapshot> {
        let user_fees_body = self
            .post_info(serde_json::json!({
                "type": "userFees",
                "user": self.account_address.as_str(),
            }))
            .await?;
        let product_body = self
            .post_info(serde_json::json!({ "type": "metaAndAssetCtxs" }))
            .await?;
        let max_age_ns = self
            .economics
            .quote_max_age_secs
            .checked_mul(crate::bolt_v3_numeric::MILLIS_PER_SECOND_U64)
            .and_then(|value| value.checked_mul(crate::bolt_v3_numeric::NANOS_PER_MILLI_U64))
            .context("Hyperliquid economics maximum age overflows nanoseconds")?;
        let valid_until_ns = refreshed_at_ns
            .checked_add(max_age_ns)
            .context("Hyperliquid economics validity deadline overflow")?;
        let user_snapshot_id = format!("sha256:{}", hex::encode(Sha256::digest(&user_fees_body)));
        let product_snapshot_id = format!("sha256:{}", hex::encode(Sha256::digest(&product_body)));
        let user_fees = HyperliquidUserFeesSnapshot::from_wire_json(
            HyperliquidSnapshotMetadata {
                snapshot_id: user_snapshot_id.clone(),
                source_at_ns: refreshed_at_ns,
                fetched_at_ns: refreshed_at_ns,
                valid_until_ns,
            },
            &self.account_id,
            std::str::from_utf8(&user_fees_body)
                .context("Hyperliquid userFees response was not UTF-8 JSON")?,
        )
        .map_err(|error| anyhow::anyhow!("invalid Hyperliquid userFees response: {error:?}"))?;
        let raw_symbol = instrument.raw_symbol();
        let carry = self
            .adapter_config
            .carry
            .as_ref()
            .context("Hyperliquid perp surface has no carry policy")?;
        let product_snapshot = HyperliquidProductEconomicsSnapshot::from_perp_meta_wire(
            HyperliquidSnapshotMetadata {
                snapshot_id: product_snapshot_id.clone(),
                source_at_ns: refreshed_at_ns,
                fetched_at_ns: refreshed_at_ns,
                valid_until_ns,
            },
            &product_body,
            raw_symbol.as_str(),
            carry,
        )
        .map_err(|error| {
            anyhow::anyhow!("invalid Hyperliquid metaAndAssetCtxs response: {error:?}")
        })?;
        let adapter = HyperliquidEconomicsAdapter::try_new(
            self.adapter_config.clone(),
            user_fees,
            product_snapshot,
        )
        .map_err(|error| anyhow::anyhow!("invalid Hyperliquid economics adapter: {error:?}"))?;
        let edge_basis_policy_id = self
            .economics
            .product_surface_policies
            .get(&self.product_surface_id)
            .context("Hyperliquid product surface has no edge-basis policy")?;
        let edge_policy = self
            .economics
            .edge_basis
            .get(edge_basis_policy_id)
            .context("Hyperliquid edge-basis policy is missing")?;
        Ok(ProviderEconomicsAuthoritySnapshot {
            product_surface_id: self.product_surface_id.clone(),
            adapter: Arc::new(adapter),
            edge_basis: AuthoritativeEdgeBasis {
                resolver_id: FormulaId::new(edge_policy.resolver_id.clone())?,
                product_metadata_source: SourceId::new(
                    edge_policy.product_metadata_source.clone(),
                )?,
                policy_version: edge_policy.policy_version,
                source_snapshot_ids: vec![SnapshotId::new(product_snapshot_id)?],
                valid_until_ns,
            },
            valuation_observations: Vec::new(),
        })
    }
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
        let product_flags_valid = match product.product_kind {
            HyperliquidProductKind::Spot => !product.hip3 && !product.growth_mode,
            HyperliquidProductKind::Perp => !product.stable_pair,
        };
        if product.deployer_scale < Decimal::ZERO
            || !product_flags_valid
            || config.formula.stable_pair_scale < Decimal::ZERO
            || config.formula.growth_mode_scale < Decimal::ZERO
            || config.formula.hip3_scale_threshold < Decimal::ZERO
            || config.formula.hip3_below_threshold_base < Decimal::ZERO
            || config.formula.hip3_at_or_above_threshold_multiplier < Decimal::ZERO
        {
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
        let oracle_price = self
            .product
            .carry_oracle_price
            .ok_or(HyperliquidEconomicsError::MissingCarryPolicy)?;
        if oracle_price <= Decimal::ZERO {
            return Err(HyperliquidEconomicsError::InvalidCarryBound);
        }
        let horizon = Decimal::from(position.holding_horizon_ns);
        let position_notional = position
            .quantity
            .checked_mul(oracle_price)
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        let stressed_position_notional = position_notional
            .checked_mul(policy.standard_price_stress_multiplier)
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        let directional_point_rate = match position.side {
            PositionSide::Long => -point_rate,
            PositionSide::Short => point_rate,
        };
        let point_amount = position_notional
            .checked_mul(directional_point_rate)
            .and_then(|amount| amount.checked_mul(horizon))
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        let debit_bound = stressed_position_notional
            .checked_mul(-bound_rate)
            .and_then(|amount| amount.checked_mul(horizon))
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        if debit_bound >= Decimal::ZERO {
            return Err(HyperliquidEconomicsError::InvalidCarryBound);
        }
        let current_adverse_projection = point_amount.min(Decimal::ZERO).abs();
        if current_adverse_projection > debit_bound.abs() {
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
                authority: RiskBoundAuthority::VenueRateCapWithPriceStress,
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
                    value: oracle_price,
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
        && user_fees.trial_escrow >= Decimal::ZERO
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
    fn resolve_edge_basis(
        &self,
        request: &EconomicQuoteRequest,
    ) -> Result<crate::economics::ResolvedEdgeBasis, EconomicsUnavailable> {
        self.validate_request(request).map_err(|_| {
            EconomicsUnavailable::ProviderQuoteUnavailable {
                source_id: self.config.source_id.clone(),
            }
        })?;
        Ok(crate::economics::ResolvedEdgeBasis {
            normalized_amount: self.notional(request).map_err(|_| {
                EconomicsUnavailable::ProviderQuoteUnavailable {
                    source_id: self.config.source_id.clone(),
                }
            })?,
            source_snapshot_ids: vec![SnapshotId::new(self.product.snapshot_id.clone())?],
            valid_until_ns: self.product.valid_until_ns,
        })
    }

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
