use std::sync::Arc;

use rust_decimal::Decimal;
use serde::Deserialize;

use crate::{
    bolt_v3_config::{EconomicsRoutingAttachmentPolicy, ExecutionEconomicsConfig},
    bolt_v3_economics_runtime::AuthoritativeVenueEconomicsInput,
    bolt_v3_providers::{BuiltProviderEconomicsAdapter, ProviderEconomicsAdapterBuildContext},
    economics::{
        AccountId, AdmissionTreatment, AssetId, CalculationFactor, CarryKind, CurrencyId,
        EconomicClass, EconomicComponentId, EconomicKind, EconomicScope, EconomicsError,
        EconomicsInstrumentId, EconomicsQuoteRequest, EstimatedEffect, ExecutionClientId,
        ExecutionKind, FormulaId, IncentiveKind, InventoryApplication, LiquidityRole, OrderSide,
        PlannedFillNotional, PointEstimate, PositionSide, ProductSurfaceId, RiskBoundAuthority,
        RoutingAttachmentId, SignedNativeEffect, SnapshotId, SourceIdentity, SourceValidity,
        VenueEconomicsAdapter, VenueEconomicsUnavailable, VenueEdgeBasisEstimate,
        VenueQuoteEstimate,
    },
};

const BASIS_POINTS_PER_UNIT: i64 = 10_000;
const NANOSECONDS_PER_SECOND: u64 = 1_000_000_000;
const SECONDS_PER_HOUR: u64 = 3_600;
const TENTHS_OF_BASIS_POINTS_PER_UNIT: i64 = 100_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidSnapshotMetadata {
    pub source: SourceIdentity,
    pub snapshot_id: SnapshotId,
    pub source_at_ns: u64,
    pub fetched_at_ns: u64,
    pub valid_until_ns: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HyperliquidReplaySnapshotMetadata {
    source: String,
    snapshot_id: String,
    source_at_ns: u64,
    fetched_at_ns: u64,
    valid_until_ns: u64,
}

impl HyperliquidReplaySnapshotMetadata {
    fn into_snapshot(self) -> Result<HyperliquidSnapshotMetadata, EconomicsError> {
        Ok(HyperliquidSnapshotMetadata {
            source: SourceIdentity::try_new(self.source)?,
            snapshot_id: SnapshotId::try_new(self.snapshot_id)?,
            source_at_ns: self.source_at_ns,
            fetched_at_ns: self.fetched_at_ns,
            valid_until_ns: self.valid_until_ns,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HyperliquidReplayJsonSnapshot {
    metadata: HyperliquidReplaySnapshotMetadata,
    json: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum HyperliquidReplayProductSnapshot {
    Perpetual {
        metadata: HyperliquidReplaySnapshotMetadata,
        deployer_fee_scale: Decimal,
        growth_mode: bool,
        #[serde(default)]
        aligned_quote: Option<HyperliquidReplayJsonSnapshot>,
        context: HyperliquidReplayJsonSnapshot,
    },
    Spot {
        metadata: HyperliquidReplaySnapshotMetadata,
        base_asset: String,
        quote_currency: String,
        stable_pair: bool,
        aligned_quote: HyperliquidReplayJsonSnapshot,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HyperliquidReplayBuilderApproval {
    metadata: HyperliquidReplaySnapshotMetadata,
    attachment_id: String,
    json: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HyperliquidReplayEconomicsAuthority {
    user_fees: HyperliquidReplayJsonSnapshot,
    account_id: String,
    product: HyperliquidReplayProductSnapshot,
    #[serde(default)]
    builder_approval: Option<HyperliquidReplayBuilderApproval>,
}

impl HyperliquidSnapshotMetadata {
    fn validity(&self) -> SourceValidity {
        SourceValidity {
            source: self.source.clone(),
            snapshot_id: self.snapshot_id.clone(),
            source_at_ns: self.source_at_ns,
            fetched_at_ns: self.fetched_at_ns,
            valid_until_ns: self.valid_until_ns,
        }
    }

    fn timeline_is_valid(&self) -> bool {
        self.source_at_ns <= self.fetched_at_ns && self.fetched_at_ns <= self.valid_until_ns
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidUserFeesSnapshot {
    metadata: HyperliquidSnapshotMetadata,
    account_id: AccountId,
    perp_taker_rate: Decimal,
    perp_maker_rate: Decimal,
    spot_taker_rate: Decimal,
    spot_maker_rate: Decimal,
    active_referral_discount: Decimal,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HyperliquidUserFeesWire {
    daily_user_vlm: serde_json::Value,
    fee_schedule: serde_json::Value,
    user_cross_rate: String,
    user_add_rate: String,
    user_spot_cross_rate: String,
    user_spot_add_rate: String,
    active_referral_discount: String,
    trial: Option<serde_json::Value>,
    #[serde(alias = "feeTrialEscrow")]
    fee_trial_reward: String,
    next_trial_available_timestamp: Option<u64>,
    staking_link: Option<serde_json::Value>,
    active_staking_discount: serde_json::Value,
}

impl HyperliquidUserFeesSnapshot {
    pub fn from_json(
        metadata: HyperliquidSnapshotMetadata,
        account_id: AccountId,
        json: &str,
    ) -> Result<Self, HyperliquidEconomicsError> {
        let wire: HyperliquidUserFeesWire =
            serde_json::from_str(json).map_err(|_| HyperliquidEconomicsError::InvalidUserFees)?;
        let parse_rate = |value: &str| {
            value
                .parse::<Decimal>()
                .map_err(|_| HyperliquidEconomicsError::InvalidUserFees)
        };
        let perp_taker_rate = parse_rate(&wire.user_cross_rate)?;
        let perp_maker_rate = parse_rate(&wire.user_add_rate)?;
        let spot_taker_rate = parse_rate(&wire.user_spot_cross_rate)?;
        let spot_maker_rate = parse_rate(&wire.user_spot_add_rate)?;
        let active_referral_discount = parse_rate(&wire.active_referral_discount)?;
        let fee_trial_reward = parse_rate(&wire.fee_trial_reward)?;
        if !metadata.timeline_is_valid()
            || !wire.daily_user_vlm.is_array()
            || !wire.fee_schedule.is_object()
            || perp_taker_rate < Decimal::ZERO
            || spot_taker_rate < Decimal::ZERO
            || active_referral_discount < Decimal::ZERO
            || active_referral_discount >= Decimal::ONE
            || fee_trial_reward < Decimal::ZERO
            || !wire.active_staking_discount.is_object()
        {
            return Err(HyperliquidEconomicsError::InvalidUserFees);
        }
        let _ = (
            wire.trial,
            wire.next_trial_available_timestamp,
            wire.staking_link,
        );
        Ok(Self {
            metadata,
            account_id,
            perp_taker_rate,
            perp_maker_rate,
            spot_taker_rate,
            spot_maker_rate,
            active_referral_discount,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HyperliquidAlignedQuoteSnapshot {
    metadata: HyperliquidSnapshotMetadata,
    is_aligned: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HyperliquidAlignedQuoteWire {
    is_aligned: bool,
    first_aligned_time: Option<u64>,
    evm_minted_supply: String,
    daily_amount_owed: Vec<[String; 2]>,
    predicted_rate: String,
}

impl HyperliquidAlignedQuoteSnapshot {
    fn from_json(
        metadata: HyperliquidSnapshotMetadata,
        json: &str,
    ) -> Result<Self, HyperliquidEconomicsError> {
        let wire: HyperliquidAlignedQuoteWire = serde_json::from_str(json)
            .map_err(|_| HyperliquidEconomicsError::UnsupportedAlignedQuoteShape)?;
        if !metadata.timeline_is_valid()
            || aligned_decimal(&wire.evm_minted_supply)? < Decimal::ZERO
            || aligned_decimal(&wire.predicted_rate)? < Decimal::ZERO
            || wire
                .daily_amount_owed
                .iter()
                .any(|entry| entry[0].trim().is_empty() || aligned_decimal(&entry[1]).is_err())
            || (wire.is_aligned && wire.first_aligned_time.is_none())
        {
            return Err(HyperliquidEconomicsError::UnsupportedAlignedQuoteShape);
        }
        Ok(Self {
            metadata,
            is_aligned: wire.is_aligned,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HyperliquidPerpContextSnapshot {
    metadata: HyperliquidSnapshotMetadata,
    funding_rate: Decimal,
    oracle_price: Decimal,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HyperliquidPerpContextWire {
    funding: String,
    open_interest: String,
    prev_day_px: String,
    day_ntl_vlm: String,
    premium: Option<String>,
    oracle_px: String,
    mark_px: String,
    mid_px: Option<String>,
    impact_pxs: Option<Vec<String>>,
    day_base_vlm: String,
}

impl HyperliquidPerpContextSnapshot {
    fn from_json(
        metadata: HyperliquidSnapshotMetadata,
        json: &str,
    ) -> Result<Self, HyperliquidEconomicsError> {
        let wire: HyperliquidPerpContextWire = serde_json::from_str(json)
            .map_err(|_| HyperliquidEconomicsError::InvalidProductMetadata)?;
        let funding_rate = product_decimal(&wire.funding)?;
        let oracle_price = product_decimal(&wire.oracle_px)?;
        let required_non_negative = [
            product_decimal(&wire.open_interest)?,
            product_decimal(&wire.prev_day_px)?,
            product_decimal(&wire.day_ntl_vlm)?,
            product_decimal(&wire.mark_px)?,
            product_decimal(&wire.day_base_vlm)?,
        ];
        if !metadata.timeline_is_valid()
            || oracle_price <= Decimal::ZERO
            || required_non_negative
                .iter()
                .any(|value| *value < Decimal::ZERO)
            || wire
                .premium
                .as_deref()
                .is_some_and(|value| product_decimal(value).is_err())
            || wire
                .mid_px
                .as_deref()
                .is_some_and(|value| product_decimal(value).is_err())
            || wire
                .impact_pxs
                .as_ref()
                .is_some_and(|values| values.iter().any(|value| product_decimal(value).is_err()))
        {
            return Err(HyperliquidEconomicsError::InvalidProductMetadata);
        }
        Ok(Self {
            metadata,
            funding_rate,
            oracle_price,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HyperliquidProductKind {
    Perpetual {
        deployer_fee_scale: Decimal,
        growth_mode: bool,
    },
    Spot {
        base_asset: AssetId,
        quote_currency: CurrencyId,
        stable_pair: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidProductEconomicsSnapshot {
    metadata: HyperliquidSnapshotMetadata,
    instrument_id: EconomicsInstrumentId,
    product_surface_id: ProductSurfaceId,
    kind: HyperliquidProductKind,
    alignment: Option<HyperliquidAlignedQuoteSnapshot>,
    perp_context: Option<HyperliquidPerpContextSnapshot>,
}

pub struct HyperliquidPerpetualSnapshotInput<'a> {
    pub metadata: HyperliquidSnapshotMetadata,
    pub instrument_id: EconomicsInstrumentId,
    pub product_surface_id: ProductSurfaceId,
    pub deployer_fee_scale: Decimal,
    pub growth_mode: bool,
    pub aligned_quote_json: Option<(&'a HyperliquidSnapshotMetadata, &'a str)>,
    pub context_metadata: HyperliquidSnapshotMetadata,
    pub context_json: &'a str,
}

impl HyperliquidProductEconomicsSnapshot {
    pub fn perp_from_json(
        input: HyperliquidPerpetualSnapshotInput<'_>,
    ) -> Result<Self, HyperliquidEconomicsError> {
        let HyperliquidPerpetualSnapshotInput {
            metadata,
            instrument_id,
            product_surface_id,
            deployer_fee_scale,
            growth_mode,
            aligned_quote_json,
            context_metadata,
            context_json,
        } = input;
        let alignment = aligned_quote_json
            .map(|(metadata, json)| {
                HyperliquidAlignedQuoteSnapshot::from_json(metadata.clone(), json)
            })
            .transpose()?;
        let snapshot = Self {
            metadata,
            instrument_id,
            product_surface_id,
            kind: HyperliquidProductKind::Perpetual {
                deployer_fee_scale,
                growth_mode,
            },
            alignment,
            perp_context: Some(HyperliquidPerpContextSnapshot::from_json(
                context_metadata,
                context_json,
            )?),
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn spot_from_json(
        metadata: HyperliquidSnapshotMetadata,
        instrument_id: EconomicsInstrumentId,
        product_surface_id: ProductSurfaceId,
        base_asset: AssetId,
        quote_currency: CurrencyId,
        stable_pair: bool,
        aligned_quote_json: (&HyperliquidSnapshotMetadata, &str),
    ) -> Result<Self, HyperliquidEconomicsError> {
        let (aligned_metadata, aligned_quote_json) = aligned_quote_json;
        let snapshot = Self {
            metadata,
            instrument_id,
            product_surface_id,
            kind: HyperliquidProductKind::Spot {
                base_asset,
                quote_currency,
                stable_pair,
            },
            alignment: Some(HyperliquidAlignedQuoteSnapshot::from_json(
                aligned_metadata.clone(),
                aligned_quote_json,
            )?),
            perp_context: None,
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    fn validate(&self) -> Result<(), HyperliquidEconomicsError> {
        if !self.metadata.timeline_is_valid() {
            return Err(HyperliquidEconomicsError::InvalidProductMetadata);
        }
        match &self.kind {
            HyperliquidProductKind::Perpetual {
                deployer_fee_scale,
                growth_mode,
            } if *deployer_fee_scale < Decimal::ZERO || self.perp_context.is_none() => {
                Err(HyperliquidEconomicsError::InvalidProductMetadata)
            }
            HyperliquidProductKind::Spot { .. }
                if self.alignment.is_none() || self.perp_context.is_some() =>
            {
                Err(HyperliquidEconomicsError::InvalidProductMetadata)
            }
            _ => Ok(()),
        }
    }

    fn is_aligned(&self) -> bool {
        self.alignment
            .as_ref()
            .is_some_and(|alignment| alignment.is_aligned)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidBuilderApprovalSnapshot {
    metadata: HyperliquidSnapshotMetadata,
    attachment_id: RoutingAttachmentId,
    max_fee_tenths_bps: u32,
}

impl HyperliquidBuilderApprovalSnapshot {
    pub fn from_json(
        metadata: HyperliquidSnapshotMetadata,
        attachment_id: RoutingAttachmentId,
        json: &str,
    ) -> Result<Self, HyperliquidEconomicsError> {
        let max_fee_tenths_bps: u32 = serde_json::from_str(json)
            .map_err(|_| HyperliquidEconomicsError::InvalidBuilderApproval)?;
        if !metadata.timeline_is_valid() {
            return Err(HyperliquidEconomicsError::InvalidBuilderApproval);
        }
        Ok(Self {
            metadata,
            attachment_id,
            max_fee_tenths_bps,
        })
    }
}

#[derive(Clone)]
struct HyperliquidAuthoritativeEconomics {
    user_fees: HyperliquidUserFeesSnapshot,
    product: HyperliquidProductEconomicsSnapshot,
    builder_approval: Option<HyperliquidBuilderApprovalSnapshot>,
}

pub fn authoritative_economics_input(
    execution_client_id: impl Into<String>,
    instrument_id: impl Into<String>,
    product_surface_id: impl Into<String>,
    user_fees: HyperliquidUserFeesSnapshot,
    product: HyperliquidProductEconomicsSnapshot,
    builder_approval: Option<HyperliquidBuilderApprovalSnapshot>,
) -> Result<AuthoritativeVenueEconomicsInput, HyperliquidEconomicsError> {
    let execution_client_id = execution_client_id.into();
    let instrument_id = EconomicsInstrumentId::try_new(instrument_id.into())?;
    let product_surface_id = ProductSurfaceId::try_new(product_surface_id.into())?;
    ExecutionClientId::try_new(execution_client_id.clone())?;
    if product.instrument_id != instrument_id || product.product_surface_id != product_surface_id {
        return Err(HyperliquidEconomicsError::InvalidRequestScope);
    }
    Ok(AuthoritativeVenueEconomicsInput::from_provider_authority(
        execution_client_id,
        instrument_id.as_str(),
        product_surface_id.as_str(),
        super::KEY,
        Arc::new(HyperliquidAuthoritativeEconomics {
            user_fees,
            product,
            builder_approval,
        }),
    ))
}

pub(crate) fn build_replay_economics_authority(
    context: crate::bolt_v3_providers::ProviderEconomicsReplayAuthorityBuildContext<'_>,
) -> Result<AuthoritativeVenueEconomicsInput, String> {
    let replay: HyperliquidReplayEconomicsAuthority = context
        .authority
        .clone()
        .try_into()
        .map_err(|error| format!("invalid Hyperliquid replay economics authority: {error}"))?;
    let user_fees = HyperliquidUserFeesSnapshot::from_json(
        replay
            .user_fees
            .metadata
            .into_snapshot()
            .map_err(|error| error.to_string())?,
        AccountId::try_new(replay.account_id).map_err(|error| error.to_string())?,
        &replay.user_fees.json,
    )
    .map_err(|error| error.to_string())?;
    let instrument_id = EconomicsInstrumentId::try_new(context.instrument_id.to_string())
        .map_err(|error| error.to_string())?;
    let product_surface_id = ProductSurfaceId::try_new(context.product_surface_id.to_string())
        .map_err(|error| error.to_string())?;
    let product = match replay.product {
        HyperliquidReplayProductSnapshot::Perpetual {
            metadata,
            deployer_fee_scale,
            growth_mode,
            aligned_quote,
            context: product_context,
        } => {
            let aligned_quote = aligned_quote
                .map(|snapshot| -> Result<_, String> {
                    Ok((
                        snapshot
                            .metadata
                            .into_snapshot()
                            .map_err(|error| error.to_string())?,
                        snapshot.json,
                    ))
                })
                .transpose()?;
            HyperliquidProductEconomicsSnapshot::perp_from_json(HyperliquidPerpetualSnapshotInput {
                metadata: metadata
                    .into_snapshot()
                    .map_err(|error| error.to_string())?,
                instrument_id,
                product_surface_id,
                deployer_fee_scale,
                growth_mode,
                aligned_quote_json: aligned_quote
                    .as_ref()
                    .map(|(metadata, json)| (metadata, json.as_str())),
                context_metadata: product_context
                    .metadata
                    .into_snapshot()
                    .map_err(|error| error.to_string())?,
                context_json: &product_context.json,
            })
        }
        HyperliquidReplayProductSnapshot::Spot {
            metadata,
            base_asset,
            quote_currency,
            stable_pair,
            aligned_quote,
        } => HyperliquidProductEconomicsSnapshot::spot_from_json(
            metadata
                .into_snapshot()
                .map_err(|error| error.to_string())?,
            instrument_id,
            product_surface_id,
            AssetId::try_new(base_asset).map_err(|error| error.to_string())?,
            CurrencyId::try_new(quote_currency).map_err(|error| error.to_string())?,
            stable_pair,
            (
                &aligned_quote
                    .metadata
                    .into_snapshot()
                    .map_err(|error| error.to_string())?,
                &aligned_quote.json,
            ),
        ),
    }
    .map_err(|error| error.to_string())?;
    let builder_approval = replay
        .builder_approval
        .map(|approval| {
            HyperliquidBuilderApprovalSnapshot::from_json(
                approval
                    .metadata
                    .into_snapshot()
                    .map_err(|error| error.to_string())?,
                RoutingAttachmentId::try_new(approval.attachment_id)
                    .map_err(|error| error.to_string())?,
                &approval.json,
            )
            .map_err(|error| error.to_string())
        })
        .transpose()?;
    authoritative_economics_input(
        context.execution_client_id,
        context.instrument_id,
        context.product_surface_id,
        user_fees,
        product,
        builder_approval,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn build_execution_economics_adapter(
    context: ProviderEconomicsAdapterBuildContext<'_>,
) -> Result<BuiltProviderEconomicsAdapter, String> {
    let authority = context
        .authority
        .downcast_ref::<HyperliquidAuthoritativeEconomics>()
        .ok_or_else(|| "Hyperliquid economics authority has the wrong snapshot type".to_string())?;
    let execution = context
        .execution
        .clone()
        .try_into::<super::HyperliquidExecutionConfig>()
        .map_err(|error| error.to_string())?;
    if execution.account_id.to_string() != authority.user_fees.account_id.as_str() {
        return Err("Hyperliquid user-fees authority belongs to another account".to_string());
    }
    let config = adapter_config_from_toml(
        context.config,
        context.product_surface_id,
        &authority.user_fees,
        &authority.product,
    )?;
    HyperliquidEconomicsAdapter::try_new(
        config,
        authority.user_fees.clone(),
        authority.product.clone(),
        authority.builder_approval.clone(),
    )
    .map(|adapter| BuiltProviderEconomicsAdapter {
        account_id: execution.account_id.to_string(),
        adapter: Arc::new(adapter),
    })
    .map_err(|error| error.to_string())
}

fn adapter_config_from_toml(
    config: &ExecutionEconomicsConfig,
    product_surface_id: &str,
    user_fees: &HyperliquidUserFeesSnapshot,
    product: &HyperliquidProductEconomicsSnapshot,
) -> Result<HyperliquidEconomicsConfig, String> {
    validate_execution_economics_config(config)?;
    let policy_id = config
        .product_surface_policies
        .get(product_surface_id)
        .ok_or_else(|| {
            format!("Hyperliquid product surface `{product_surface_id}` has no edge-basis policy")
        })?;
    let edge_basis = config
        .edge_basis
        .get(policy_id)
        .ok_or_else(|| format!("Hyperliquid edge-basis policy `{policy_id}` is not configured"))?;
    if product.product_surface_id.as_str() != product_surface_id
        || user_fees.metadata.source.as_str() != config.sources["account_fees"]
        || product.metadata.source.as_str() != config.sources["product_metadata"]
        || edge_basis.product_metadata_source != config.sources["product_metadata"]
    {
        return Err("Hyperliquid authoritative source identity does not match TOML".to_string());
    }
    let carry = if config.carry_surfaces.contains(product_surface_id) {
        let carry = config
            .carry
            .as_ref()
            .ok_or_else(|| "Hyperliquid carry surface has no carry policy".to_string())?;
        let context = product
            .perp_context
            .as_ref()
            .ok_or_else(|| "Hyperliquid carry surface has no funding context".to_string())?;
        if context.metadata.source.as_str() != config.sources["funding"] {
            return Err("Hyperliquid funding authority does not match TOML".to_string());
        }
        Some(carry_policy_from_toml(carry)?)
    } else {
        None
    };
    let protocol = &config.quote_components["protocol"];
    let settlement = &config.assets["settlement"];
    Ok(HyperliquidEconomicsConfig {
        settlement_currency: config_id(&settlement.currency, CurrencyId::try_new)?,
        protocol_component_id: config_id(&protocol.component_id, EconomicComponentId::try_new)?,
        protocol_formula_id: config_id(&protocol.formula_id, FormulaId::try_new)?,
        protocol_rate_factor_id: config_id(&protocol.rate_factor_id, FormulaId::try_new)?,
        routing: None,
        stable_pair_scale: config_decimal(config, "stable_pair_scale")?,
        growth_mode_scale: config_decimal(config, "growth_mode_scale")?,
        hip3_scale_threshold: config_decimal(config, "hip3_scale_threshold")?,
        hip3_below_threshold_base: config_decimal(config, "hip3_below_threshold_base")?,
        hip3_at_or_above_threshold_multiplier: config_decimal(
            config,
            "hip3_at_or_above_threshold_multiplier",
        )?,
        edge_basis_resolver_id: config_id(&edge_basis.resolver_id, FormulaId::try_new)?,
        edge_basis_product_metadata_source: config_id(
            &edge_basis.product_metadata_source,
            SourceIdentity::try_new,
        )?,
        edge_basis_policy_version: edge_basis.policy_version,
        carry,
    })
}

pub(crate) fn validate_execution_economics_config(
    config: &ExecutionEconomicsConfig,
) -> Result<(), String> {
    if config.routing_attachment_policy != EconomicsRoutingAttachmentPolicy::Forbidden {
        return Err("Hyperliquid Slice 1 requires routing attachments to be forbidden".to_string());
    }
    let expected_sources = if config.carry_surfaces.is_empty() {
        ["account_fees", "product_metadata"].as_slice()
    } else {
        ["account_fees", "funding", "product_metadata"].as_slice()
    };
    if config
        .sources
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        != expected_sources
        || config
            .formula
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != [
                "growth_mode_scale",
                "hip3_at_or_above_threshold_multiplier",
                "hip3_below_threshold_base",
                "hip3_scale_threshold",
                "stable_pair_scale",
            ]
        || config
            .quote_components
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            != ["protocol"]
        || config.assets.keys().map(String::as_str).collect::<Vec<_>>() != ["settlement"]
        || config.carry.is_some() != !config.carry_surfaces.is_empty()
        || config
            .carry_surfaces
            .iter()
            .any(|surface| !config.product_surface_policies.contains_key(surface))
    {
        return Err("Hyperliquid economics contains unsupported authority keys".to_string());
    }
    let stable_pair_scale = config_decimal(config, "stable_pair_scale")?;
    let growth_mode_scale = config_decimal(config, "growth_mode_scale")?;
    let hip3_scale_threshold = config_decimal(config, "hip3_scale_threshold")?;
    let hip3_below_threshold_base = config_decimal(config, "hip3_below_threshold_base")?;
    let hip3_at_or_above_threshold_multiplier =
        config_decimal(config, "hip3_at_or_above_threshold_multiplier")?;
    if stable_pair_scale < Decimal::ZERO
        || growth_mode_scale < Decimal::ZERO
        || hip3_scale_threshold <= Decimal::ZERO
        || hip3_below_threshold_base < Decimal::ZERO
        || hip3_at_or_above_threshold_multiplier < Decimal::ZERO
    {
        return Err("Hyperliquid economics formula values are invalid".to_string());
    }
    let protocol = &config.quote_components["protocol"];
    config_id(&protocol.component_id, EconomicComponentId::try_new)?;
    config_id(&protocol.formula_id, FormulaId::try_new)?;
    config_id(&protocol.rate_factor_id, FormulaId::try_new)?;
    config_id(&config.assets["settlement"].currency, CurrencyId::try_new)?;
    let product_metadata_source = &config.sources["product_metadata"];
    for edge_basis in config.edge_basis.values() {
        config_id(&edge_basis.resolver_id, FormulaId::try_new)?;
        if &edge_basis.product_metadata_source != product_metadata_source {
            return Err(
                "Hyperliquid edge-basis metadata source must match product_metadata".to_string(),
            );
        }
    }
    if let Some(carry) = &config.carry {
        carry_policy_from_toml(carry)?;
    }
    Ok(())
}

fn carry_policy_from_toml(
    carry: &crate::bolt_v3_config::EconomicsCarryPolicyConfig,
) -> Result<HyperliquidCarryPolicy, String> {
    let funding_interval_ns = carry
        .funding_interval_secs
        .checked_mul(NANOSECONDS_PER_SECOND)
        .ok_or_else(|| "Hyperliquid funding interval overflows nanoseconds".to_string())?;
    let funding_schedule_phase_ns = carry
        .funding_schedule_phase_secs
        .checked_mul(NANOSECONDS_PER_SECOND)
        .ok_or_else(|| "Hyperliquid funding phase overflows nanoseconds".to_string())?;
    let hourly_bps = carry
        .standard_stress
        .venue_rate_cap_bps_per_hour
        .parse::<Decimal>()
        .map_err(|_| "Hyperliquid funding rate cap is invalid".to_string())?;
    let debit_rate_bound_per_interval = hourly_bps
        .checked_div(Decimal::from(BASIS_POINTS_PER_UNIT))
        .and_then(|value| value.checked_mul(Decimal::from(carry.funding_interval_secs)))
        .and_then(|value| value.checked_div(Decimal::from(SECONDS_PER_HOUR)))
        .ok_or_else(|| "Hyperliquid funding rate cap overflows".to_string())?;
    let stress = &carry.standard_stress;
    Ok(HyperliquidCarryPolicy {
        component_id: config_id(&carry.component_id, EconomicComponentId::try_new)?,
        formula_id: config_id(&carry.formula_id, FormulaId::try_new)?,
        point_rate_factor_id: config_id(&carry.point_rate_factor_id, FormulaId::try_new)?,
        debit_bound_rate_factor_id: config_id(&carry.bound_rate_factor_id, FormulaId::try_new)?,
        oracle_price_factor_id: config_id(&carry.oracle_price_factor_id, FormulaId::try_new)?,
        event_count_factor_id: config_id(&carry.event_count_factor_id, FormulaId::try_new)?,
        price_stress_factor_id: config_id(&stress.price_multiplier_factor_id, FormulaId::try_new)?,
        risk_policy_id: config_id(&carry.risk_policy_id, FormulaId::try_new)?,
        next_funding_at_factor_id: config_id(&carry.next_funding_at_factor_id, FormulaId::try_new)?,
        stress_artifact_id: config_id(&stress.artifact_id, SourceIdentity::try_new)?,
        stress_artifact_version: stress.artifact_version.get(),
        stress_artifact_version_factor_id: config_id(
            &stress.artifact_version_factor_id,
            FormulaId::try_new,
        )?,
        funding_interval_ns,
        funding_schedule_phase_ns,
        debit_rate_bound_per_interval,
        price_stress_multiplier: stress
            .price_multiplier
            .parse()
            .map_err(|_| "Hyperliquid funding price multiplier is invalid".to_string())?,
    })
}

fn config_decimal(config: &ExecutionEconomicsConfig, key: &str) -> Result<Decimal, String> {
    config.formula[key]
        .parse()
        .map_err(|_| format!("Hyperliquid formula `{key}` is invalid"))
}

fn config_id<T>(
    value: &str,
    constructor: impl FnOnce(String) -> Result<T, EconomicsError>,
) -> Result<T, String> {
    constructor(value.to_string()).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidCarryPolicy {
    pub component_id: EconomicComponentId,
    pub formula_id: FormulaId,
    pub point_rate_factor_id: FormulaId,
    pub debit_bound_rate_factor_id: FormulaId,
    pub oracle_price_factor_id: FormulaId,
    pub event_count_factor_id: FormulaId,
    pub price_stress_factor_id: FormulaId,
    pub risk_policy_id: FormulaId,
    pub next_funding_at_factor_id: FormulaId,
    pub stress_artifact_id: SourceIdentity,
    pub stress_artifact_version: u64,
    pub stress_artifact_version_factor_id: FormulaId,
    pub funding_interval_ns: u64,
    pub funding_schedule_phase_ns: u64,
    pub debit_rate_bound_per_interval: Decimal,
    pub price_stress_multiplier: Decimal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidEconomicsConfig {
    pub settlement_currency: CurrencyId,
    pub protocol_component_id: EconomicComponentId,
    pub protocol_formula_id: FormulaId,
    pub protocol_rate_factor_id: FormulaId,
    pub routing: Option<HyperliquidRoutingEconomicsConfig>,
    pub stable_pair_scale: Decimal,
    pub growth_mode_scale: Decimal,
    pub hip3_scale_threshold: Decimal,
    pub hip3_below_threshold_base: Decimal,
    pub hip3_at_or_above_threshold_multiplier: Decimal,
    pub edge_basis_resolver_id: FormulaId,
    pub edge_basis_product_metadata_source: SourceIdentity,
    pub edge_basis_policy_version: u64,
    pub carry: Option<HyperliquidCarryPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidRoutingEconomicsConfig {
    pub component_id: EconomicComponentId,
    pub formula_id: FormulaId,
    pub rate_factor_id: FormulaId,
    pub attachment_id: RoutingAttachmentId,
    pub fee_tenths_bps: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HyperliquidEconomicsError {
    InvalidUserFees,
    InvalidProductMetadata,
    UnsupportedAlignedQuoteShape,
    InvalidBuilderApproval,
    InvalidRequestScope,
    MissingCarryContext,
    InvalidCarryBound,
    MissingBuilderApproval,
    BuilderApprovalInsufficient,
    UnsupportedAttachedBuilderForSpotBuy,
    ArithmeticOverflow,
    InvalidEffect(EconomicsError),
}

impl std::fmt::Display for HyperliquidEconomicsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUserFees => f.write_str("Hyperliquid user-fees authority is invalid"),
            Self::InvalidProductMetadata => {
                f.write_str("Hyperliquid product economics metadata is invalid")
            }
            Self::UnsupportedAlignedQuoteShape => {
                f.write_str("Hyperliquid aligned-quote response shape is unsupported")
            }
            Self::InvalidBuilderApproval => f.write_str("Hyperliquid builder approval is invalid"),
            Self::InvalidRequestScope => {
                f.write_str("Hyperliquid quote request does not match its runtime authority")
            }
            Self::MissingCarryContext => {
                f.write_str("Hyperliquid perp quote is missing position holding context")
            }
            Self::InvalidCarryBound => f.write_str("Hyperliquid funding bound is invalid"),
            Self::MissingBuilderApproval => {
                f.write_str("Hyperliquid attached builder has no approval authority")
            }
            Self::BuilderApprovalInsufficient => {
                f.write_str("Hyperliquid builder fee exceeds the approved maximum")
            }
            Self::UnsupportedAttachedBuilderForSpotBuy => {
                f.write_str("Hyperliquid spot buys do not support attached builder charges")
            }
            Self::ArithmeticOverflow => f.write_str("Hyperliquid economics arithmetic overflowed"),
            Self::InvalidEffect(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for HyperliquidEconomicsError {}

impl From<EconomicsError> for HyperliquidEconomicsError {
    fn from(value: EconomicsError) -> Self {
        Self::InvalidEffect(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HyperliquidEconomicsAdapter {
    config: HyperliquidEconomicsConfig,
    user_fees: HyperliquidUserFeesSnapshot,
    product: HyperliquidProductEconomicsSnapshot,
    builder_approval: Option<HyperliquidBuilderApprovalSnapshot>,
}

impl HyperliquidEconomicsAdapter {
    pub fn try_new(
        config: HyperliquidEconomicsConfig,
        user_fees: HyperliquidUserFeesSnapshot,
        product: HyperliquidProductEconomicsSnapshot,
        builder_approval: Option<HyperliquidBuilderApprovalSnapshot>,
    ) -> Result<Self, HyperliquidEconomicsError> {
        product.validate()?;
        if !user_fees.metadata.timeline_is_valid()
            || config
                .routing
                .as_ref()
                .is_some_and(|routing| routing.fee_tenths_bps == 0)
            || config.stable_pair_scale < Decimal::ZERO
            || config.growth_mode_scale < Decimal::ZERO
            || config.hip3_scale_threshold <= Decimal::ZERO
            || config.hip3_below_threshold_base < Decimal::ZERO
            || config.hip3_at_or_above_threshold_multiplier < Decimal::ZERO
            || config.carry.as_ref().is_some_and(|carry| {
                carry.funding_interval_ns == 0
                    || carry.funding_schedule_phase_ns >= carry.funding_interval_ns
                    || carry.debit_rate_bound_per_interval <= Decimal::ZERO
                    || carry.price_stress_multiplier < Decimal::ONE
                    || carry.stress_artifact_version == 0
            })
            || (matches!(product.kind, HyperliquidProductKind::Perpetual { .. })
                && config.carry.is_none())
            || match (config.routing.as_ref(), builder_approval.as_ref()) {
                (_, None) => false,
                (Some(routing), Some(approval)) => {
                    !approval.metadata.timeline_is_valid()
                        || approval.attachment_id != routing.attachment_id
                }
                (None, Some(_)) => true,
            }
        {
            return Err(HyperliquidEconomicsError::InvalidProductMetadata);
        }
        if product.is_aligned() {
            return Err(HyperliquidEconomicsError::UnsupportedAlignedQuoteShape);
        }
        Ok(Self {
            config,
            user_fees,
            product,
            builder_approval,
        })
    }

    pub fn quote(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<VenueQuoteEstimate, HyperliquidEconomicsError> {
        request.validate()?;
        if request.account_id != self.user_fees.account_id
            || request.instrument_id != self.product.instrument_id
            || request.product_surface_id != self.product.product_surface_id
        {
            return Err(HyperliquidEconomicsError::InvalidRequestScope);
        }
        let (maker_rate, taker_rate) = self.effective_rates()?;
        let rate = match request.liquidity_role {
            LiquidityRole::GuaranteedMaker => maker_rate,
            LiquidityRole::Taker => taker_rate,
        };
        let mut components = Vec::new();
        if !rate.is_zero() {
            components.push(self.protocol_effect(request, rate)?);
        }
        let mut dependencies = vec![self.product.metadata.validity()];
        if let Some(context) = &self.product.perp_context {
            dependencies.push(context.metadata.validity());
        }
        if let Some(alignment) = &self.product.alignment {
            dependencies.push(alignment.metadata.validity());
        }
        if self.builder_applies(request)? {
            let routing = self
                .config
                .routing
                .as_ref()
                .ok_or(HyperliquidEconomicsError::InvalidRequestScope)?;
            let approval = self
                .builder_approval
                .as_ref()
                .ok_or(HyperliquidEconomicsError::MissingBuilderApproval)?;
            if routing.fee_tenths_bps > approval.max_fee_tenths_bps {
                return Err(HyperliquidEconomicsError::BuilderApprovalInsufficient);
            }
            dependencies.push(approval.metadata.validity());
            components.push(self.builder_effect(request, approval)?);
        }
        if matches!(self.product.kind, HyperliquidProductKind::Perpetual { .. })
            && let Some(carry) = self.carry_effect(request)?
        {
            components.push(carry);
        }
        Ok(VenueQuoteEstimate {
            authority: self.user_fees.metadata.validity(),
            dependency_sources: dependencies,
            components,
        })
    }

    fn effective_rates(&self) -> Result<(Decimal, Decimal), HyperliquidEconomicsError> {
        let (mut maker, mut taker, stable_scale, deployer_scale, growth_scale) =
            match &self.product.kind {
                HyperliquidProductKind::Perpetual {
                    deployer_fee_scale,
                    growth_mode,
                } => (
                    self.user_fees.perp_maker_rate,
                    self.user_fees.perp_taker_rate,
                    Decimal::ONE,
                    *deployer_fee_scale,
                    if *growth_mode {
                        self.config.growth_mode_scale
                    } else {
                        Decimal::ONE
                    },
                ),
                HyperliquidProductKind::Spot { stable_pair, .. } => (
                    self.user_fees.spot_maker_rate,
                    self.user_fees.spot_taker_rate,
                    if *stable_pair {
                        self.config.stable_pair_scale
                    } else {
                        Decimal::ONE
                    },
                    Decimal::ZERO,
                    Decimal::ONE,
                ),
            };
        let hip3_scale = if deployer_scale < self.config.hip3_scale_threshold {
            self.config
                .hip3_below_threshold_base
                .checked_add(deployer_scale)
                .ok_or(HyperliquidEconomicsError::ArithmeticOverflow)?
        } else {
            deployer_scale
                .checked_mul(self.config.hip3_at_or_above_threshold_multiplier)
                .ok_or(HyperliquidEconomicsError::ArithmeticOverflow)?
        };
        maker = maker
            .checked_mul(stable_scale)
            .and_then(|value| value.checked_mul(growth_scale))
            .ok_or(HyperliquidEconomicsError::ArithmeticOverflow)?;
        if maker > Decimal::ZERO {
            maker = maker
                .checked_mul(hip3_scale)
                .and_then(|value| {
                    value.checked_mul(Decimal::ONE - self.user_fees.active_referral_discount)
                })
                .ok_or(HyperliquidEconomicsError::ArithmeticOverflow)?;
        }
        taker = taker
            .checked_mul(stable_scale)
            .and_then(|value| value.checked_mul(hip3_scale))
            .and_then(|value| value.checked_mul(growth_scale))
            .and_then(|value| {
                value.checked_mul(Decimal::ONE - self.user_fees.active_referral_discount)
            })
            .ok_or(HyperliquidEconomicsError::ArithmeticOverflow)?;
        Ok((maker, taker))
    }

    fn protocol_effect(
        &self,
        request: &EconomicsQuoteRequest,
        rate: Decimal,
    ) -> Result<EstimatedEffect, HyperliquidEconomicsError> {
        let effect = match (&self.product.kind, request.order_side) {
            (HyperliquidProductKind::Spot { base_asset, .. }, OrderSide::Buy) => {
                let quantity = request
                    .planned_fill_legs
                    .iter()
                    .try_fold(Decimal::ZERO, |total, leg| total.checked_add(leg.quantity))
                    .ok_or(HyperliquidEconomicsError::ArithmeticOverflow)?;
                SignedNativeEffect::asset(
                    negate_product(quantity, rate)?,
                    base_asset.clone(),
                    InventoryApplication::AlreadyAppliedToGrossFill,
                )?
            }
            (HyperliquidProductKind::Spot { quote_currency, .. }, OrderSide::Sell) => {
                SignedNativeEffect::currency(
                    negate_product(
                        PlannedFillNotional::from_legs(&request.planned_fill_legs)?.amount(),
                        rate,
                    )?,
                    quote_currency.clone(),
                )?
            }
            (HyperliquidProductKind::Perpetual { .. }, _) => SignedNativeEffect::currency(
                negate_product(
                    PlannedFillNotional::from_legs(&request.planned_fill_legs)?.amount(),
                    rate,
                )?,
                self.config.settlement_currency.clone(),
            )?,
        };
        let amount = effect.amount();
        Ok(EstimatedEffect {
            component_id: self.config.protocol_component_id.clone(),
            class: effect_class(amount),
            kind: if amount.is_sign_positive() {
                EconomicKind::Incentive(IncentiveKind::MakerRebate)
            } else {
                EconomicKind::Execution(ExecutionKind::ProtocolTrading)
            },
            scope: decision_scope(request),
            point_estimate: PointEstimate::NonZero(effect),
            debit_risk_bound: None,
            admission_treatment: AdmissionTreatment::GuaranteedConditionalOnAction,
            calculation_factors: vec![CalculationFactor {
                factor_id: self.config.protocol_rate_factor_id.clone(),
                value: rate,
            }],
            formula_id: self.config.protocol_formula_id.clone(),
            source: self.user_fees.metadata.validity(),
        })
    }

    fn builder_applies(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<bool, HyperliquidEconomicsError> {
        let Some(attached) = &request.routing.attached_charge else {
            return Ok(false);
        };
        let routing = self
            .config
            .routing
            .as_ref()
            .ok_or(HyperliquidEconomicsError::InvalidRequestScope)?;
        if attached != &routing.attachment_id {
            return Err(HyperliquidEconomicsError::InvalidRequestScope);
        }
        if matches!(
            (&self.product.kind, request.order_side),
            (HyperliquidProductKind::Spot { .. }, OrderSide::Buy)
        ) {
            return Err(HyperliquidEconomicsError::UnsupportedAttachedBuilderForSpotBuy);
        }
        Ok(true)
    }

    fn builder_effect(
        &self,
        request: &EconomicsQuoteRequest,
        approval: &HyperliquidBuilderApprovalSnapshot,
    ) -> Result<EstimatedEffect, HyperliquidEconomicsError> {
        let routing = self
            .config
            .routing
            .as_ref()
            .ok_or(HyperliquidEconomicsError::InvalidRequestScope)?;
        let amount = PlannedFillNotional::from_legs(&request.planned_fill_legs)?
            .amount()
            .checked_mul(Decimal::from(routing.fee_tenths_bps))
            .and_then(|value| value.checked_div(Decimal::from(TENTHS_OF_BASIS_POINTS_PER_UNIT)))
            .and_then(|value| Decimal::ZERO.checked_sub(value))
            .ok_or(HyperliquidEconomicsError::ArithmeticOverflow)?;
        let currency = match &self.product.kind {
            HyperliquidProductKind::Perpetual { .. } => self.config.settlement_currency.clone(),
            HyperliquidProductKind::Spot { quote_currency, .. } => quote_currency.clone(),
        };
        Ok(EstimatedEffect {
            component_id: routing.component_id.clone(),
            class: EconomicClass::Charge,
            kind: EconomicKind::Execution(ExecutionKind::AttachedRoutingCharge),
            scope: decision_scope(request),
            point_estimate: PointEstimate::NonZero(SignedNativeEffect::currency(amount, currency)?),
            debit_risk_bound: None,
            admission_treatment: AdmissionTreatment::GuaranteedConditionalOnAction,
            calculation_factors: vec![CalculationFactor {
                factor_id: routing.rate_factor_id.clone(),
                value: Decimal::from(routing.fee_tenths_bps),
            }],
            formula_id: routing.formula_id.clone(),
            source: approval.metadata.validity(),
        })
    }

    fn carry_effect(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<Option<EstimatedEffect>, HyperliquidEconomicsError> {
        let position = request
            .position
            .as_ref()
            .ok_or(HyperliquidEconomicsError::MissingCarryContext)?;
        let context = self
            .product
            .perp_context
            .as_ref()
            .ok_or(HyperliquidEconomicsError::InvalidProductMetadata)?;
        let holding_until_ns = request
            .requested_at_ns
            .checked_add(position.holding_horizon_ns)
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        let policy = self
            .config
            .carry
            .as_ref()
            .ok_or(HyperliquidEconomicsError::InvalidProductMetadata)?;
        let next_funding_at_ns = next_funding_at(
            request.requested_at_ns,
            policy.funding_interval_ns,
            policy.funding_schedule_phase_ns,
        )?;
        let event_count = if holding_until_ns < next_funding_at_ns {
            0
        } else {
            holding_until_ns
                .checked_sub(next_funding_at_ns)
                .and_then(|elapsed| elapsed.checked_div(policy.funding_interval_ns))
                .and_then(|events| events.checked_add(1))
                .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?
        };
        let event_count_decimal = Decimal::from(event_count);
        if event_count == 0 {
            return Ok(None);
        }
        let position_notional = position
            .quantity
            .checked_mul(context.oracle_price)
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        let directional_rate = match position.side {
            PositionSide::Long => -context.funding_rate,
            PositionSide::Short => context.funding_rate,
        };
        let point = position_notional
            .checked_mul(directional_rate)
            .and_then(|value| value.checked_mul(event_count_decimal))
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        let debit_bound = position_notional
            .checked_mul(policy.price_stress_multiplier)
            .and_then(|value| value.checked_mul(-policy.debit_rate_bound_per_interval))
            .and_then(|value| value.checked_mul(event_count_decimal))
            .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
        if debit_bound >= Decimal::ZERO || point.min(Decimal::ZERO).abs() > debit_bound.abs() {
            return Err(HyperliquidEconomicsError::InvalidCarryBound);
        }
        let point_estimate = if point.is_zero() {
            PointEstimate::ProvenZero {
                factor_id: policy.point_rate_factor_id.clone(),
            }
        } else {
            PointEstimate::NonZero(SignedNativeEffect::currency(
                point,
                self.config.settlement_currency.clone(),
            )?)
        };
        Ok(Some(EstimatedEffect {
            component_id: policy.component_id.clone(),
            class: if point.is_sign_positive() {
                EconomicClass::Credit
            } else {
                EconomicClass::Charge
            },
            kind: EconomicKind::Carry(CarryKind::Funding),
            scope: EconomicScope::PositionInterval {
                position_id: position.position_id.clone(),
                starts_at_ns: request.requested_at_ns,
                ends_at_ns: holding_until_ns,
            },
            point_estimate,
            debit_risk_bound: Some(SignedNativeEffect::currency(
                debit_bound,
                self.config.settlement_currency.clone(),
            )?),
            admission_treatment: AdmissionTreatment::RiskBound {
                authority: RiskBoundAuthority::VenueRateCapWithPriceStress,
            },
            calculation_factors: vec![
                CalculationFactor {
                    factor_id: policy.point_rate_factor_id.clone(),
                    value: context.funding_rate,
                },
                CalculationFactor {
                    factor_id: policy.debit_bound_rate_factor_id.clone(),
                    value: policy.debit_rate_bound_per_interval,
                },
                CalculationFactor {
                    factor_id: policy.oracle_price_factor_id.clone(),
                    value: context.oracle_price,
                },
                CalculationFactor {
                    factor_id: policy.event_count_factor_id.clone(),
                    value: event_count_decimal,
                },
                CalculationFactor {
                    factor_id: policy.next_funding_at_factor_id.clone(),
                    value: Decimal::from(next_funding_at_ns),
                },
                CalculationFactor {
                    factor_id: policy.price_stress_factor_id.clone(),
                    value: policy.price_stress_multiplier,
                },
                CalculationFactor {
                    factor_id: policy.stress_artifact_version_factor_id.clone(),
                    value: Decimal::from(policy.stress_artifact_version),
                },
            ],
            formula_id: policy.formula_id.clone(),
            source: context.metadata.validity(),
        }))
    }
}

impl VenueEconomicsAdapter for HyperliquidEconomicsAdapter {
    fn provider_key(&self) -> &str {
        super::KEY
    }

    fn resolve_edge_basis(
        &self,
        request: &EconomicsQuoteRequest,
        planned_fill_notional: PlannedFillNotional,
    ) -> Result<VenueEdgeBasisEstimate, VenueEconomicsUnavailable> {
        if request.account_id != self.user_fees.account_id
            || request.instrument_id != self.product.instrument_id
            || request.product_surface_id != self.product.product_surface_id
        {
            return Err(VenueEconomicsUnavailable::RequestScopeMismatch);
        }
        Ok(VenueEdgeBasisEstimate {
            resolver_id: self.config.edge_basis_resolver_id.clone(),
            product_metadata_source: self.config.edge_basis_product_metadata_source.clone(),
            policy_version: self.config.edge_basis_policy_version,
            normalized_amount: crate::economics::EdgeBasisAmount::try_new(
                planned_fill_notional.amount(),
            )?,
            source_snapshot_ids: vec![self.product.metadata.snapshot_id.clone()],
            valid_until_ns: self.product.metadata.valid_until_ns,
        })
    }

    fn quote(
        &self,
        request: &EconomicsQuoteRequest,
    ) -> Result<VenueQuoteEstimate, VenueEconomicsUnavailable> {
        HyperliquidEconomicsAdapter::quote(self, request).map_err(|error| match error {
            HyperliquidEconomicsError::InvalidRequestScope => {
                VenueEconomicsUnavailable::RequestScopeMismatch
            }
            HyperliquidEconomicsError::UnsupportedAlignedQuoteShape
            | HyperliquidEconomicsError::UnsupportedAttachedBuilderForSpotBuy => {
                VenueEconomicsUnavailable::UnsupportedProductEconomics
            }
            HyperliquidEconomicsError::MissingBuilderApproval => {
                VenueEconomicsUnavailable::MissingAuthoritativeSnapshot
            }
            HyperliquidEconomicsError::InvalidEffect(error) => error.into(),
            HyperliquidEconomicsError::InvalidUserFees
            | HyperliquidEconomicsError::InvalidProductMetadata
            | HyperliquidEconomicsError::InvalidBuilderApproval
            | HyperliquidEconomicsError::MissingCarryContext
            | HyperliquidEconomicsError::InvalidCarryBound
            | HyperliquidEconomicsError::BuilderApprovalInsufficient
            | HyperliquidEconomicsError::ArithmeticOverflow => {
                VenueEconomicsUnavailable::InvalidAuthoritativeSnapshot
            }
        })
    }
}

fn product_decimal(value: &str) -> Result<Decimal, HyperliquidEconomicsError> {
    value
        .parse()
        .map_err(|_| HyperliquidEconomicsError::InvalidProductMetadata)
}

fn aligned_decimal(value: &str) -> Result<Decimal, HyperliquidEconomicsError> {
    value
        .parse()
        .map_err(|_| HyperliquidEconomicsError::UnsupportedAlignedQuoteShape)
}

fn negate_product(amount: Decimal, rate: Decimal) -> Result<Decimal, HyperliquidEconomicsError> {
    amount
        .checked_mul(rate)
        .and_then(|value| Decimal::ZERO.checked_sub(value))
        .ok_or(HyperliquidEconomicsError::ArithmeticOverflow)
}

fn next_funding_at(
    requested_at_ns: u64,
    interval_ns: u64,
    phase_ns: u64,
) -> Result<u64, HyperliquidEconomicsError> {
    if requested_at_ns < phase_ns {
        return Ok(phase_ns);
    }
    let elapsed = requested_at_ns
        .checked_sub(phase_ns)
        .ok_or(HyperliquidEconomicsError::InvalidCarryBound)?;
    elapsed
        .checked_div(interval_ns)
        .and_then(|period| period.checked_add(1))
        .and_then(|period| period.checked_mul(interval_ns))
        .and_then(|offset| phase_ns.checked_add(offset))
        .ok_or(HyperliquidEconomicsError::InvalidCarryBound)
}

fn effect_class(amount: Decimal) -> EconomicClass {
    if amount.is_sign_positive() {
        EconomicClass::Credit
    } else {
        EconomicClass::Charge
    }
}

fn decision_scope(request: &EconomicsQuoteRequest) -> EconomicScope {
    EconomicScope::Decision {
        decision_correlation_id: request.decision_correlation_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::economics::{
        ActionId, DecisionCorrelationId, EdgeBasisPolicyId, ExecutionClientId, LifecyclePath,
        NativeUnitId, PlannedFillLeg, PositionContext, PositionId, ReportingPolicyId,
        RoutingContext, validate_and_aggregate_quote,
    };

    fn id<T>(value: &str, constructor: impl FnOnce(String) -> Result<T, EconomicsError>) -> T {
        constructor(value.to_owned()).expect("fixture identifier should be canonical")
    }

    fn metadata(source: &str, snapshot: &str) -> HyperliquidSnapshotMetadata {
        HyperliquidSnapshotMetadata {
            source: id(source, SourceIdentity::try_new),
            snapshot_id: id(snapshot, SnapshotId::try_new),
            source_at_ns: 900,
            fetched_at_ns: 950,
            valid_until_ns: 1_100,
        }
    }

    fn config() -> HyperliquidEconomicsConfig {
        HyperliquidEconomicsConfig {
            settlement_currency: id("hUSD", CurrencyId::try_new),
            protocol_component_id: id("protocol", EconomicComponentId::try_new),
            protocol_formula_id: id("protocol-v1", FormulaId::try_new),
            protocol_rate_factor_id: id("protocol-rate", FormulaId::try_new),
            routing: Some(HyperliquidRoutingEconomicsConfig {
                component_id: id("builder", EconomicComponentId::try_new),
                formula_id: id("builder-v1", FormulaId::try_new),
                rate_factor_id: id("builder-rate", FormulaId::try_new),
                attachment_id: id("builder-code", RoutingAttachmentId::try_new),
                fee_tenths_bps: 10,
            }),
            stable_pair_scale: Decimal::new(2, 1),
            growth_mode_scale: Decimal::new(1, 1),
            hip3_scale_threshold: Decimal::ONE,
            hip3_below_threshold_base: Decimal::ONE,
            hip3_at_or_above_threshold_multiplier: Decimal::from(2),
            edge_basis_resolver_id: id("product-metadata", FormulaId::try_new),
            edge_basis_product_metadata_source: id(
                "hyperliquid-product-metadata",
                SourceIdentity::try_new,
            ),
            edge_basis_policy_version: 1,
            carry: Some(HyperliquidCarryPolicy {
                component_id: id("funding", EconomicComponentId::try_new),
                formula_id: id("funding-v1", FormulaId::try_new),
                point_rate_factor_id: id("funding-point-rate", FormulaId::try_new),
                debit_bound_rate_factor_id: id("funding-bound-rate", FormulaId::try_new),
                oracle_price_factor_id: id("funding-oracle", FormulaId::try_new),
                event_count_factor_id: id("funding-events", FormulaId::try_new),
                price_stress_factor_id: id("funding-stress", FormulaId::try_new),
                risk_policy_id: id("funding-risk-policy", FormulaId::try_new),
                next_funding_at_factor_id: id("funding-next-event", FormulaId::try_new),
                stress_artifact_id: id("funding-stress-artifact", SourceIdentity::try_new),
                stress_artifact_version: 1,
                stress_artifact_version_factor_id: id("funding-stress-version", FormulaId::try_new),
                funding_interval_ns: 100,
                funding_schedule_phase_ns: 0,
                debit_rate_bound_per_interval: Decimal::new(1, 3),
                price_stress_multiplier: Decimal::new(12, 1),
            }),
        }
    }

    fn user_fees(metadata: HyperliquidSnapshotMetadata) -> HyperliquidUserFeesSnapshot {
        HyperliquidUserFeesSnapshot::from_json(
            metadata,
            id("account", AccountId::try_new),
            include_str!("../../../tests/fixtures/economics/hyperliquid/user_fees_discounted.json"),
        )
        .expect("user-fees fixture should parse")
    }

    fn perp_product() -> HyperliquidProductEconomicsSnapshot {
        HyperliquidProductEconomicsSnapshot::perp_from_json(HyperliquidPerpetualSnapshotInput {
            metadata: metadata("meta-and-asset-contexts", "perp-1"),
            instrument_id: id("BTC-PERP", EconomicsInstrumentId::try_new),
            product_surface_id: id("perpetual", ProductSurfaceId::try_new),
            deployer_fee_scale: Decimal::ZERO,
            growth_mode: false,
            aligned_quote_json: None,
            context_metadata: metadata("funding-context", "funding-1"),
            context_json: include_str!(
                "../../../tests/fixtures/economics/hyperliquid/perp_context.json"
            ),
        })
        .expect("perp fixture should parse")
    }

    fn spot_product() -> HyperliquidProductEconomicsSnapshot {
        let unaligned =
            include_str!("../../../tests/fixtures/economics/hyperliquid/aligned_quote.json")
                .replacen("\"isAligned\": true", "\"isAligned\": false", 1);
        HyperliquidProductEconomicsSnapshot::spot_from_json(
            metadata("spot-meta", "spot-1"),
            id("HYPE-hUSD", EconomicsInstrumentId::try_new),
            id("spot", ProductSurfaceId::try_new),
            id("HYPE", AssetId::try_new),
            id("hUSD", CurrencyId::try_new),
            false,
            (&metadata("aligned-quote", "aligned-1"), &unaligned),
        )
        .expect("spot fixture should parse")
    }

    fn approval() -> HyperliquidBuilderApprovalSnapshot {
        HyperliquidBuilderApprovalSnapshot::from_json(
            metadata("max-builder-fee", "builder-1"),
            id("builder-code", RoutingAttachmentId::try_new),
            include_str!("../../../tests/fixtures/economics/hyperliquid/builder_approval.json"),
        )
        .expect("approval fixture should parse")
    }

    fn request(
        instrument: &str,
        surface: &str,
        side: OrderSide,
        role: LiquidityRole,
        routed: bool,
        with_position: bool,
    ) -> EconomicsQuoteRequest {
        EconomicsQuoteRequest {
            execution_client_id: id("execution", ExecutionClientId::try_new),
            account_id: id("account", AccountId::try_new),
            instrument_id: id(instrument, EconomicsInstrumentId::try_new),
            product_surface_id: id(surface, ProductSurfaceId::try_new),
            order_side: side,
            liquidity_role: role,
            planned_fill_legs: vec![
                PlannedFillLeg {
                    price: Decimal::from(4),
                    quantity: Decimal::from(10),
                },
                PlannedFillLeg {
                    price: Decimal::from(5),
                    quantity: Decimal::from(20),
                },
            ],
            routing: RoutingContext {
                attached_charge: routed.then(|| id("builder-code", RoutingAttachmentId::try_new)),
            },
            position: with_position.then(|| PositionContext {
                position_id: id("position", PositionId::try_new),
                side: PositionSide::Long,
                quantity: Decimal::from(2),
                holding_horizon_ns: 250,
            }),
            lifecycle_path: LifecyclePath::Transfer {
                action_id: id("action", ActionId::try_new),
            },
            reporting_policy_id: id("reporting", ReportingPolicyId::try_new),
            reporting_currency: id("hUSD", CurrencyId::try_new),
            edge_basis_policy_id: id("basis", EdgeBasisPolicyId::try_new),
            requested_at_ns: 1_000,
            decision_correlation_id: id("decision", DecisionCorrelationId::try_new),
        }
    }

    #[test]
    fn perp_taker_fee_and_long_funding_are_separate_and_bounded() {
        let adapter = HyperliquidEconomicsAdapter::try_new(
            config(),
            user_fees(metadata("user-fees", "fees-1")),
            perp_product(),
            None,
        )
        .expect("perp adapter should construct");
        let request = request(
            "BTC-PERP",
            "perpetual",
            OrderSide::Buy,
            LiquidityRole::Taker,
            false,
            true,
        );
        let estimate = adapter.quote(&request).expect("perp quote should resolve");
        assert_eq!(estimate.components.len(), 2);
        let funding = estimate
            .components
            .iter()
            .find(|component| component.kind == EconomicKind::Carry(CarryKind::Funding))
            .expect("funding component should exist");
        assert_eq!(
            funding
                .debit_risk_bound
                .as_ref()
                .map(SignedNativeEffect::amount),
            Some(Decimal::new(-48, 2))
        );
        assert_eq!(
            funding.point_estimate,
            PointEstimate::NonZero(
                SignedNativeEffect::currency(Decimal::new(-4, 2), id("hUSD", CurrencyId::try_new))
                    .unwrap()
            )
        );
        assert_eq!(funding.source.source.as_str(), "funding-context");
        let quote = validate_and_aggregate_quote(&request, estimate, &[])
            .expect("same-currency perp quote should aggregate");
        assert_eq!(quote.core_total(), Decimal::new(-522336, 6));
    }

    #[test]
    fn negative_maker_rate_is_a_guaranteed_credit_after_user_discounts() {
        let adapter = HyperliquidEconomicsAdapter::try_new(
            config(),
            user_fees(metadata("user-fees", "fees-1")),
            perp_product(),
            None,
        )
        .expect("perp adapter should construct");
        let request = request(
            "BTC-PERP",
            "perpetual",
            OrderSide::Sell,
            LiquidityRole::GuaranteedMaker,
            false,
            true,
        );
        let estimate = adapter.quote(&request).expect("maker quote should resolve");
        let rebate = estimate
            .components
            .iter()
            .find(|component| component.kind == EconomicKind::Incentive(IncentiveKind::MakerRebate))
            .expect("negative maker rate should be a rebate");
        assert_eq!(
            rebate.point_estimate,
            PointEstimate::NonZero(
                SignedNativeEffect::currency(Decimal::new(14, 4), id("hUSD", CurrencyId::try_new))
                    .unwrap()
            )
        );
        assert_eq!(
            rebate.admission_treatment,
            AdmissionTreatment::GuaranteedConditionalOnAction
        );
    }

    #[test]
    fn spot_fee_uses_base_on_buy_and_quote_on_sell() {
        let adapter = HyperliquidEconomicsAdapter::try_new(
            config(),
            user_fees(metadata("user-fees", "fees-1")),
            spot_product(),
            None,
        )
        .expect("spot adapter should construct");
        let buy = request(
            "HYPE-hUSD",
            "spot",
            OrderSide::Buy,
            LiquidityRole::Taker,
            false,
            false,
        );
        let buy_estimate = adapter.quote(&buy).expect("spot buy should quote");
        let buy_effect = match &buy_estimate.components[0].point_estimate {
            PointEstimate::NonZero(effect) => effect,
            PointEstimate::ProvenZero { .. } => panic!("spot fee must be non-zero"),
        };
        assert_eq!(
            buy_effect.unit(),
            NativeUnitId::Asset(id("HYPE", AssetId::try_new))
        );
        assert_eq!(buy_effect.amount(), Decimal::new(-14112, 6));

        let sell = request(
            "HYPE-hUSD",
            "spot",
            OrderSide::Sell,
            LiquidityRole::Taker,
            false,
            false,
        );
        let sell_estimate = adapter.quote(&sell).expect("spot sell should quote");
        let sell_effect = match &sell_estimate.components[0].point_estimate {
            PointEstimate::NonZero(effect) => effect,
            PointEstimate::ProvenZero { .. } => panic!("spot fee must be non-zero"),
        };
        assert_eq!(
            sell_effect.unit(),
            NativeUnitId::Currency(id("hUSD", CurrencyId::try_new))
        );
        assert_eq!(sell_effect.amount(), Decimal::new(-65856, 6));
    }

    #[test]
    fn aligned_product_is_blocked_without_authoritative_benefit_version() {
        let product = HyperliquidProductEconomicsSnapshot::spot_from_json(
            metadata("spot-meta", "spot-1"),
            id("HYPE-hUSD", EconomicsInstrumentId::try_new),
            id("spot", ProductSurfaceId::try_new),
            id("HYPE", AssetId::try_new),
            id("hUSD", CurrencyId::try_new),
            false,
            (
                &metadata("aligned-quote", "aligned-1"),
                include_str!("../../../tests/fixtures/economics/hyperliquid/aligned_quote.json"),
            ),
        )
        .expect("captured aligned-status fixture should parse");

        assert_eq!(
            HyperliquidEconomicsAdapter::try_new(
                config(),
                user_fees(metadata("user-fees", "fees-1")),
                product,
                None,
            ),
            Err(HyperliquidEconomicsError::UnsupportedAlignedQuoteShape)
        );
    }

    #[test]
    fn builder_requires_current_approval_and_rejects_unsupported_spot_buys() {
        let adapter = HyperliquidEconomicsAdapter::try_new(
            config(),
            user_fees(metadata("user-fees", "fees-1")),
            spot_product(),
            Some(approval()),
        )
        .expect("routed spot adapter should construct");
        let buy = request(
            "HYPE-hUSD",
            "spot",
            OrderSide::Buy,
            LiquidityRole::Taker,
            true,
            false,
        );
        assert_eq!(
            adapter.quote(&buy),
            Err(HyperliquidEconomicsError::UnsupportedAttachedBuilderForSpotBuy)
        );
        let sell = request(
            "HYPE-hUSD",
            "spot",
            OrderSide::Sell,
            LiquidityRole::Taker,
            true,
            false,
        );
        let sell_estimate = adapter.quote(&sell).expect("spot sell should quote");
        assert!(
            sell_estimate
                .components
                .iter()
                .any(|component| component.kind
                    == EconomicKind::Execution(ExecutionKind::AttachedRoutingCharge))
        );

        let unapproved = HyperliquidEconomicsAdapter::try_new(
            config(),
            user_fees(metadata("user-fees", "fees-1")),
            perp_product(),
            None,
        )
        .expect("unrouted perp adapter should construct");
        assert_eq!(
            unapproved.quote(&request(
                "BTC-PERP",
                "perpetual",
                OrderSide::Buy,
                LiquidityRole::Taker,
                true,
                true
            )),
            Err(HyperliquidEconomicsError::MissingBuilderApproval)
        );
    }

    #[test]
    fn routing_forbidden_config_has_no_latent_builder_authority() {
        let mut config = config();
        config.routing = None;
        let adapter = HyperliquidEconomicsAdapter::try_new(
            config,
            user_fees(metadata("user-fees", "fees-1")),
            spot_product(),
            None,
        )
        .expect("unrouted spot adapter should construct without builder authority");
        assert!(
            adapter
                .quote(&request(
                    "HYPE-hUSD",
                    "spot",
                    OrderSide::Sell,
                    LiquidityRole::Taker,
                    false,
                    false,
                ))
                .is_ok()
        );
        assert_eq!(
            adapter.quote(&request(
                "HYPE-hUSD",
                "spot",
                OrderSide::Sell,
                LiquidityRole::Taker,
                true,
                false,
            )),
            Err(HyperliquidEconomicsError::InvalidRequestScope)
        );
    }

    #[test]
    fn missing_stale_and_divergent_authority_fail_closed() {
        let fixture =
            include_str!("../../../tests/fixtures/economics/hyperliquid/user_fees_discounted.json");
        let mut missing: serde_json::Value = serde_json::from_str(fixture).unwrap();
        missing.as_object_mut().unwrap().remove("userCrossRate");
        assert_eq!(
            HyperliquidUserFeesSnapshot::from_json(
                metadata("user-fees", "fees-1"),
                id("account", AccountId::try_new),
                &missing.to_string()
            ),
            Err(HyperliquidEconomicsError::InvalidUserFees)
        );

        let aligned =
            include_str!("../../../tests/fixtures/economics/hyperliquid/aligned_quote.json");
        let mut divergent: serde_json::Value = serde_json::from_str(aligned).unwrap();
        divergent
            .as_object_mut()
            .unwrap()
            .insert("newShape".to_owned(), serde_json::Value::Bool(true));
        assert_eq!(
            HyperliquidAlignedQuoteSnapshot::from_json(
                metadata("aligned", "aligned-1"),
                &divergent.to_string()
            ),
            Err(HyperliquidEconomicsError::UnsupportedAlignedQuoteShape)
        );

        let mut stale = metadata("user-fees", "fees-stale");
        stale.valid_until_ns = 999;
        let adapter =
            HyperliquidEconomicsAdapter::try_new(config(), user_fees(stale), perp_product(), None)
                .unwrap();
        let request = request(
            "BTC-PERP",
            "perpetual",
            OrderSide::Buy,
            LiquidityRole::Taker,
            false,
            true,
        );
        assert!(matches!(
            validate_and_aggregate_quote(&request, adapter.quote(&request).unwrap(), &[]),
            Err(EconomicsError::StaleSource { .. })
        ));
    }

    #[test]
    fn funding_point_cannot_exceed_its_configured_debit_bound() {
        let mut config = config();
        config
            .carry
            .as_mut()
            .expect("fixture should configure carry")
            .debit_rate_bound_per_interval = Decimal::new(1, 5);
        let adapter = HyperliquidEconomicsAdapter::try_new(
            config,
            user_fees(metadata("user-fees", "fees-1")),
            perp_product(),
            None,
        )
        .expect("structurally valid policy should construct");
        assert_eq!(
            adapter.quote(&request(
                "BTC-PERP",
                "perpetual",
                OrderSide::Buy,
                LiquidityRole::Taker,
                false,
                true
            )),
            Err(HyperliquidEconomicsError::InvalidCarryBound)
        );
    }

    #[test]
    fn horizon_before_next_funding_has_no_carry_effect() {
        let adapter = HyperliquidEconomicsAdapter::try_new(
            config(),
            user_fees(metadata("user-fees", "fees-1")),
            perp_product(),
            None,
        )
        .expect("perp adapter should construct");
        let mut request = request(
            "BTC-PERP",
            "perpetual",
            OrderSide::Buy,
            LiquidityRole::Taker,
            false,
            true,
        );
        request
            .position
            .as_mut()
            .expect("fixture should carry a position")
            .holding_horizon_ns = 50;
        let estimate = adapter.quote(&request).expect("short horizon should quote");
        assert!(
            estimate
                .components
                .iter()
                .all(|component| component.kind != EconomicKind::Carry(CarryKind::Funding))
        );
    }
}
