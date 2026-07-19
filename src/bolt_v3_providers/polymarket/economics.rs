use crate::{
    bolt_v3_economics_runtime::{
        AuthoritativeEdgeBasis, AuthoritativeValuationObservation, EconomicsReceiptClock,
        ProviderEconomicsAuthority, ProviderEconomicsAuthorityRefresh,
        ProviderEconomicsAuthoritySnapshot, capture_economics_source_receipt,
    },
    bolt_v3_numeric::NANOS_PER_SECOND_U64,
    economics::{
        AdmissionTreatment, CalculationFactor, EconomicClass, EconomicKind, EconomicQuoteRequest,
        EconomicScope, EconomicsUnavailable, EstimatedEconomicComponent, ExecutionKind, FormulaId,
        LiquidityRoleAssumption, NativeUnitId, PointEstimate, SignedNativeEffect, SnapshotId,
        SourceId, SourceValidity, VenueEconomicsAdapter, VenueQuoteEstimate,
    },
};
use alloy_primitives::keccak256;
use anyhow::Context;
use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use nautilus_core::consts::NAUTILUS_USER_AGENT;
use nautilus_model::{
    identifiers::{InstrumentId, Venue},
    instruments::{Instrument, InstrumentAny},
};
use nautilus_network::http::{HttpClient, USER_AGENT, Url};
use nautilus_polymarket::providers::extract_condition_id;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

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
    pub source_id: SourceId,
    pub formula: PolymarketFormulaPolicy,
}

impl PolymarketEconomicsAdapterConfig {
    pub fn from_execution_config(
        economics: &crate::bolt_v3_economics_config::ExecutionEconomicsConfig,
    ) -> Result<Self, PolymarketEconomicsError> {
        let platform = economics
            .quote_components
            .get("platform")
            .ok_or(PolymarketEconomicsError::InvalidIdentity)?;
        let collateral = economics
            .assets
            .get("collateral")
            .ok_or(PolymarketEconomicsError::InvalidIdentity)?;
        if collateral.identity_kind
            != crate::bolt_v3_economics_config::EconomicsAssetIdentityKind::Currency
        {
            return Err(PolymarketEconomicsError::InvalidIdentity);
        }
        let rounding_mode = match economics
            .formula
            .get("fee_rounding_mode")
            .map(String::as_str)
        {
            Some("midpoint_away_from_zero") => FeeRoundingMode::MidpointAwayFromZero,
            Some("to_zero") => FeeRoundingMode::ToZero,
            _ => return Err(PolymarketEconomicsError::InvalidIdentity),
        };
        Ok(Self {
            collateral_unit: NativeUnitId::new(collateral.native_unit.clone())
                .map_err(|_| PolymarketEconomicsError::InvalidIdentity)?,
            platform_component_id: crate::economics::EconomicComponentId::new(
                platform.component_id.clone(),
            )
            .map_err(|_| PolymarketEconomicsError::InvalidIdentity)?,
            platform_formula_id: FormulaId::new(platform.formula_id.clone())
                .map_err(|_| PolymarketEconomicsError::InvalidIdentity)?,
            platform_rate_factor_id: FormulaId::new(platform.rate_factor_id.clone())
                .map_err(|_| PolymarketEconomicsError::InvalidIdentity)?,
            source_id: SourceId::new(
                economics
                    .sources
                    .get("schedule")
                    .ok_or(PolymarketEconomicsError::InvalidIdentity)?
                    .clone(),
            )
            .map_err(|_| PolymarketEconomicsError::InvalidIdentity)?,
            formula: PolymarketFormulaPolicy {
                fee_round_decimal_places: economics
                    .formula
                    .get("fee_round_decimal_places")
                    .and_then(|value| u32::from_str(value).ok())
                    .ok_or(PolymarketEconomicsError::InvalidIdentity)?,
                fee_rounding_mode: rounding_mode,
            },
        })
    }
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
pub enum PolymarketEconomicsError {
    InvalidMarketInfo,
    MissingFeeDescriptor,
    UnsupportedExponent,
    InvalidRate,
    InvalidFillLeg,
    StaleSnapshot,
    AttachedRoutingUnsupported,
    InvalidIdentity,
    InvalidEffect,
}

pub struct PolymarketEconomicsAdapter {
    config: PolymarketEconomicsAdapterConfig,
    snapshot: PolymarketMarketInfoSnapshot,
    platform_plan: PlatformQuotePlan,
}

#[async_trait(?Send)]
pub(crate) trait PolymarketEconomicsSource: Send + Sync {
    async fn fetch_market_info_body(
        &self,
        authority: &PolymarketEconomicsAuthority,
        instrument_id: InstrumentId,
    ) -> anyhow::Result<Vec<u8>>;

    async fn observe_collateral_redemption(
        &self,
        authority: &PolymarketEconomicsAuthority,
        receipt_clock: &dyn EconomicsReceiptClock,
        max_age_ns: u64,
    ) -> anyhow::Result<AuthoritativeValuationObservation>;
}

pub(crate) struct PolymarketEconomicsSourceOverride {
    pub source: Arc<dyn PolymarketEconomicsSource>,
}

struct LivePolymarketEconomicsSource;

pub struct PolymarketEconomicsAuthority {
    execution_client_id: String,
    venue: Venue,
    economics: crate::bolt_v3_economics_config::ExecutionEconomicsConfig,
    adapter_config: PolymarketEconomicsAdapterConfig,
    product_surface_id: String,
    edge_basis_policy_id: String,
    base_url: Url,
    http_timeout_secs: u64,
    http_client: HttpClient,
    on_chain_collateral: super::PolymarketOnChainCollateralConfig,
    collateral_source_id: String,
    source: Arc<dyn PolymarketEconomicsSource>,
}

impl PolymarketEconomicsAuthority {
    pub fn try_new(
        execution_client_id: &str,
        venue: Venue,
        execution: super::PolymarketExecutionConfig,
    ) -> anyhow::Result<Self> {
        let adapter_config =
            PolymarketEconomicsAdapterConfig::from_execution_config(&execution.economics)
                .map_err(|error| anyhow::anyhow!("invalid economics adapter config: {error:?}"))?;
        let on_chain_collateral = execution
            .on_chain_collateral
            .clone()
            .context("Polymarket economics requires on-chain collateral authority")?;
        let collateral_source_id = execution
            .economics
            .assets
            .iter()
            .find(|(_, asset)| asset.native_unit == adapter_config.collateral_unit.as_str())
            .map(|(asset_id, _)| asset_id.clone())
            .context("Polymarket economics collateral identity is missing")?;
        let (product_surface_id, edge_basis_policy_id) = execution
            .economics
            .single_product_surface_binding()
            .map_err(|error| anyhow::anyhow!("invalid product surface binding: {error:?}"))?;
        let product_surface_id = product_surface_id.to_string();
        let edge_basis_policy_id = edge_basis_policy_id.to_string();
        let base_url = Url::parse(&execution.base_url_http)
            .context("invalid configured Polymarket HTTP base URL")?;
        let http_client = HttpClient::new(
            HashMap::from([(USER_AGENT.to_string(), NAUTILUS_USER_AGENT.to_string())]),
            Vec::new(),
            Vec::new(),
            None,
            Some(execution.http_timeout_secs),
            None,
        )
        .context("could not build Polymarket economics HTTP client")?;
        Ok(Self {
            execution_client_id: execution_client_id.to_string(),
            venue,
            economics: execution.economics,
            adapter_config,
            product_surface_id,
            edge_basis_policy_id,
            base_url,
            http_timeout_secs: execution.http_timeout_secs,
            http_client,
            on_chain_collateral,
            collateral_source_id,
            source: Arc::new(LivePolymarketEconomicsSource),
        })
    }

    pub(crate) fn try_new_with_source(
        execution_client_id: &str,
        venue: Venue,
        execution: super::PolymarketExecutionConfig,
        source: Arc<dyn PolymarketEconomicsSource>,
    ) -> anyhow::Result<Self> {
        let mut authority = Self::try_new(execution_client_id, venue, execution)?;
        authority.source = source;
        Ok(authority)
    }

    fn market_info_url(&self, instrument_id: &InstrumentId) -> anyhow::Result<Url> {
        let condition_id = extract_condition_id(instrument_id)
            .context("Polymarket instrument has no condition identifier")?;
        self.base_url
            .join(&format!("clob-markets/{condition_id}"))
            .context("configured Polymarket market-info URL is invalid")
    }

    async fn observe_collateral_redemption_live(
        &self,
        receipt_clock: &dyn EconomicsReceiptClock,
        max_age_ns: u64,
    ) -> anyhow::Result<AuthoritativeValuationObservation> {
        let rpc = super::collateral_accounting_source::OnChainCollateralRpcClient::try_new(
            &self.on_chain_collateral,
            self.http_timeout_secs,
        )
        .map_err(|error| anyhow::anyhow!(error))?;
        let chain_id = rpc
            .chain_id()
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        anyhow::ensure!(
            chain_id == self.on_chain_collateral.chain_id,
            "Polymarket collateral RPC returned chain {chain_id}, expected {}",
            self.on_chain_collateral.chain_id
        );
        let latest_block_number = rpc
            .block_number()
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        let block_number = latest_block_number
            .checked_sub(self.on_chain_collateral.finality_confirmations)
            .context("Polymarket collateral RPC has insufficient finalized history")?;
        let block = rpc
            .block_header(block_number)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        anyhow::ensure!(
            block.number == block_number,
            "Polymarket collateral block mismatch"
        );
        let block_tag = format!("0x{block_number:x}");
        let collateral_token =
            normalized_address(&self.on_chain_collateral.collateral_token_address)?;
        let offramp = normalized_address(&self.on_chain_collateral.collateral_offramp_address)?;
        let redemption_asset =
            normalized_address(&self.on_chain_collateral.redemption_asset_address)?;
        let configured_collateral = rpc
            .eth_call_u256_word_at(
                &offramp,
                &function_calldata("COLLATERAL_TOKEN()", None),
                &block_tag,
            )
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        anyhow::ensure!(
            address_word(&configured_collateral) == collateral_token,
            "Polymarket collateral offramp token does not match configured collateral"
        );
        let configured_redemption_asset = rpc
            .eth_call_u256_word_at(
                &collateral_token,
                &function_calldata("USDC()", None),
                &block_tag,
            )
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        anyhow::ensure!(
            address_word(&configured_redemption_asset) == redemption_asset,
            "Polymarket collateral redemption asset does not match configured asset"
        );
        let paused = rpc
            .eth_call_u256_word_at(
                &offramp,
                &function_calldata("paused(address)", Some(&redemption_asset)),
                &block_tag,
            )
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        anyhow::ensure!(
            paused.iter().all(|byte| *byte == 0),
            "Polymarket collateral redemption is paused"
        );
        let collateral_decimals = rpc
            .eth_call_u256_word_at(
                &collateral_token,
                &function_calldata("decimals()", None),
                &block_tag,
            )
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        let redemption_decimals = rpc
            .eth_call_u256_word_at(
                &redemption_asset,
                &function_calldata("decimals()", None),
                &block_tag,
            )
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        anyhow::ensure!(
            collateral_decimals == redemption_decimals,
            "Polymarket collateral and redemption asset decimals differ"
        );
        let proxy_code = rpc
            .code_at(&collateral_token, &block_tag)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        let implementation = normalized_address(
            &self
                .on_chain_collateral
                .collateral_token_implementation_address,
        )?;
        let implementation_slot = rpc
            .storage_word_at(
                &collateral_token,
                "0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc",
                &block_tag,
            )
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        anyhow::ensure!(
            address_word(&implementation_slot) == implementation,
            "Polymarket collateral proxy implementation does not match configured authority"
        );
        let implementation_code = rpc
            .code_at(&implementation, &block_tag)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        let offramp_code = rpc
            .code_at(&offramp, &block_tag)
            .await
            .map_err(|error| anyhow::anyhow!(error))?;
        let proxy_code_sha256 = sha256_hex(&proxy_code);
        let implementation_code_sha256 = sha256_hex(&implementation_code);
        let offramp_code_sha256 = sha256_hex(&offramp_code);
        anyhow::ensure!(
            proxy_code_sha256 == self.on_chain_collateral.collateral_token_proxy_code_sha256,
            "Polymarket collateral proxy bytecode is not the governed authority"
        );
        anyhow::ensure!(
            implementation_code_sha256
                == self
                    .on_chain_collateral
                    .collateral_token_implementation_code_sha256,
            "Polymarket collateral implementation bytecode is not the governed authority"
        );
        anyhow::ensure!(
            offramp_code_sha256 == self.on_chain_collateral.collateral_offramp_code_sha256,
            "Polymarket collateral offramp bytecode is not the governed authority"
        );
        let rate = Decimal::from_str(&self.on_chain_collateral.redemption_rate)
            .context("invalid configured Polymarket collateral redemption rate")?;
        let observed_at_ns = block
            .timestamp_secs
            .checked_mul(NANOS_PER_SECOND_U64)
            .context("Polymarket collateral block timestamp overflows nanoseconds")?;
        let receipt = capture_economics_source_receipt(receipt_clock, max_age_ns)?;
        let fetched_at_ns = receipt.fetched_at_ns;
        let valid_until_ns = receipt.valid_until_ns;
        anyhow::ensure!(
            observed_at_ns <= fetched_at_ns && fetched_at_ns <= valid_until_ns,
            "Polymarket collateral redemption timeline is invalid"
        );
        let proof = CollateralRedemptionProof {
            chain_id,
            block_number,
            block_hash: block.hash,
            latest_block_number,
            finality_confirmations: self.on_chain_collateral.finality_confirmations,
            collateral_token,
            offramp,
            redemption_asset,
            collateral_decimals: hex::encode(collateral_decimals),
            proxy_code_sha256,
            implementation,
            implementation_code_sha256,
            offramp_code_sha256,
            redemption_semantics_source_commit: self
                .on_chain_collateral
                .redemption_semantics_source_commit
                .clone(),
            redemption_rate: rate.to_string(),
            observed_at_ns,
            fetched_at_ns,
            valid_until_ns,
        };
        let encoded = serde_json::to_vec(&proof)
            .context("could not encode Polymarket collateral redemption proof")?;
        Ok(AuthoritativeValuationObservation::ProviderConversion {
            source_id: self.collateral_source_id.clone(),
            from_unit: self.adapter_config.collateral_unit.clone(),
            to_unit: NativeUnitId::new(self.on_chain_collateral.redemption_asset_unit.clone())?,
            rate,
            snapshot_id: SnapshotId::new(format!(
                "sha256:{}",
                hex::encode(Sha256::digest(encoded))
            ))?,
            observed_at_ns,
            fetched_at_ns,
            valid_until_ns,
        })
    }

    async fn refresh_market_info(
        &self,
        instrument: InstrumentAny,
        receipt_clock: &dyn EconomicsReceiptClock,
        max_age_ns: u64,
    ) -> anyhow::Result<PolymarketMarketAuthorityPart> {
        let instrument_id = instrument.id();
        let body = tokio::time::timeout(
            Duration::from_secs(self.economics.quote_refresh_secs),
            self.source.fetch_market_info_body(self, instrument_id),
        )
        .await
        .context("Polymarket economics market-info exceeded its refresh deadline")??;
        let receipt = capture_economics_source_receipt(receipt_clock, max_age_ns)?;
        let body_json = std::str::from_utf8(&body)
            .context("Polymarket economics market-info was not UTF-8 JSON")?;
        let snapshot_id = format!("sha256:{}", hex::encode(Sha256::digest(&body)));
        let snapshot = PolymarketMarketInfoSnapshot::from_wire_json(
            PolymarketSnapshotMetadata {
                snapshot_id: snapshot_id.clone(),
                source_at_ns: receipt.fetched_at_ns,
                fetched_at_ns: receipt.fetched_at_ns,
                valid_until_ns: receipt.valid_until_ns,
            },
            body_json,
        )
        .map_err(|error| anyhow::anyhow!("invalid Polymarket market-info: {error:?}"))?;
        let adapter = PolymarketEconomicsAdapter::try_new(self.adapter_config.clone(), snapshot)
            .map_err(|error| anyhow::anyhow!("invalid Polymarket economics adapter: {error:?}"))?;
        Ok(PolymarketMarketAuthorityPart {
            adapter,
            snapshot_id,
            fetched_at_ns: receipt.fetched_at_ns,
            valid_until_ns: receipt.valid_until_ns,
        })
    }
}

#[async_trait(?Send)]
impl PolymarketEconomicsSource for LivePolymarketEconomicsSource {
    async fn fetch_market_info_body(
        &self,
        authority: &PolymarketEconomicsAuthority,
        instrument_id: InstrumentId,
    ) -> anyhow::Result<Vec<u8>> {
        let response = authority
            .http_client
            .get(
                authority.market_info_url(&instrument_id)?.to_string(),
                None,
                None,
                Some(authority.http_timeout_secs),
                None,
            )
            .await
            .context("Polymarket economics market-info fetch failed")?;
        anyhow::ensure!(
            response.status.is_success(),
            "Polymarket economics market-info returned HTTP status {}",
            response.status.as_u16()
        );
        Ok(response.body.to_vec())
    }

    async fn observe_collateral_redemption(
        &self,
        authority: &PolymarketEconomicsAuthority,
        receipt_clock: &dyn EconomicsReceiptClock,
        max_age_ns: u64,
    ) -> anyhow::Result<AuthoritativeValuationObservation> {
        authority
            .observe_collateral_redemption_live(receipt_clock, max_age_ns)
            .await
    }
}

struct PolymarketMarketAuthorityPart {
    adapter: PolymarketEconomicsAdapter,
    snapshot_id: String,
    fetched_at_ns: u64,
    valid_until_ns: u64,
}

#[derive(Serialize)]
struct CollateralRedemptionProof {
    chain_id: u64,
    block_number: u64,
    block_hash: String,
    latest_block_number: u64,
    finality_confirmations: u64,
    collateral_token: String,
    offramp: String,
    redemption_asset: String,
    collateral_decimals: String,
    proxy_code_sha256: String,
    implementation: String,
    implementation_code_sha256: String,
    offramp_code_sha256: String,
    redemption_semantics_source_commit: String,
    redemption_rate: String,
    observed_at_ns: u64,
    fetched_at_ns: u64,
    valid_until_ns: u64,
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn normalized_address(value: &str) -> anyhow::Result<String> {
    super::collateral_accounting_source::normalized_evm_address(value)
        .map_err(|error| anyhow::anyhow!(error))
}

fn function_calldata(signature: &str, address: Option<&str>) -> String {
    let selector = keccak256(signature.as_bytes());
    let mut calldata = hex::encode(&selector[..4]);
    if let Some(address) = address {
        calldata.push_str(&"0".repeat(24));
        calldata.push_str(address);
    }
    format!("0x{calldata}")
}

fn address_word(word: &[u8; 32]) -> String {
    hex::encode(&word[12..])
}

#[async_trait(?Send)]
impl ProviderEconomicsAuthority for PolymarketEconomicsAuthority {
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

    async fn refresh_batch(
        &self,
        instruments: Vec<InstrumentAny>,
        receipt_clock: &dyn EconomicsReceiptClock,
    ) -> anyhow::Result<Vec<ProviderEconomicsAuthorityRefresh>> {
        if instruments.is_empty() {
            return Ok(Vec::new());
        }
        let max_age_ns = self
            .economics
            .quote_max_age_secs
            .checked_mul(crate::bolt_v3_numeric::MILLIS_PER_SECOND_U64)
            .and_then(|value| value.checked_mul(crate::bolt_v3_numeric::NANOS_PER_MILLI_U64))
            .context("Polymarket economics maximum age overflows nanoseconds")?;
        let edge_policy = self
            .economics
            .edge_basis
            .get(&self.edge_basis_policy_id)
            .context("Polymarket economics edge-basis policy is missing")?;
        let market_info = async {
            stream::iter(instruments.into_iter().map(|instrument| async move {
                let instrument_id = instrument.id();
                (
                    instrument_id,
                    self.refresh_market_info(instrument, receipt_clock, max_age_ns)
                        .await,
                )
            }))
            .buffer_unordered(self.economics.refresh_max_concurrency.get())
            .collect::<Vec<_>>()
            .await
        };
        let collateral = async {
            tokio::time::timeout(
                Duration::from_secs(self.economics.quote_refresh_secs),
                self.source
                    .observe_collateral_redemption(self, receipt_clock, max_age_ns),
            )
            .await
            .context("Polymarket collateral authority exceeded its refresh deadline")?
            .context("Polymarket collateral redemption authority is unavailable")
        };
        let (market_results, valuation_observation) = tokio::join!(market_info, collateral);
        let valuation_observation = valuation_observation?;
        Ok(market_results
            .into_iter()
            .map(|(instrument_id, market_result)| {
                let snapshot = market_result.and_then(|market| {
                    Ok(ProviderEconomicsAuthoritySnapshot {
                        refreshed_at_ns: market
                            .fetched_at_ns
                            .max(valuation_observation.fetched_at_ns()),
                        product_surface_id: self.product_surface_id.clone(),
                        adapter: Arc::new(market.adapter),
                        edge_basis: AuthoritativeEdgeBasis {
                            resolver_id: FormulaId::new(edge_policy.resolver_id.clone())?,
                            product_metadata_source: SourceId::new(
                                edge_policy.product_metadata_source.clone(),
                            )?,
                            policy_version: edge_policy.policy_version,
                            source_snapshot_ids: vec![SnapshotId::new(market.snapshot_id)?],
                            valid_until_ns: market.valid_until_ns,
                        },
                        valuation_observations: vec![valuation_observation.clone()],
                    })
                });
                ProviderEconomicsAuthorityRefresh {
                    instrument_id,
                    snapshot,
                }
            })
            .collect())
    }
}

enum PlatformQuotePlan {
    FeeFree,
    PriceShaped {
        rate: Decimal,
        exponent: u32,
        taker_only: bool,
    },
}

impl PolymarketEconomicsAdapter {
    pub fn try_new(
        config: PolymarketEconomicsAdapterConfig,
        snapshot: PolymarketMarketInfoSnapshot,
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
            (Some(descriptor), Some(_), Some(_)) if descriptor.e != 1 => {
                return Err(PolymarketEconomicsError::UnsupportedExponent);
            }
            (Some(descriptor), Some(_), Some(_)) if descriptor.r < Decimal::ZERO => {
                return Err(PolymarketEconomicsError::InvalidRate);
            }
            (Some(descriptor), Some(_), Some(_)) if !descriptor.to => {
                return Err(PolymarketEconomicsError::InvalidMarketInfo);
            }
            (Some(descriptor), Some(_), Some(_)) => PlatformQuotePlan::PriceShaped {
                rate: descriptor.r,
                exponent: descriptor.e,
                taker_only: descriptor.to,
            },
            _ => return Err(PolymarketEconomicsError::InvalidMarketInfo),
        };
        Ok(Self {
            config,
            snapshot,
            platform_plan,
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

        if request.routing.attached_charge.is_some() {
            return Err(PolymarketEconomicsError::AttachedRoutingUnsupported);
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
                if exponent != 1 {
                    return Err(PolymarketEconomicsError::UnsupportedExponent);
                }
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
            point_estimate: PointEstimate::NonZero(point_effect),
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
    fn resolve_edge_basis(
        &self,
        request: &EconomicQuoteRequest,
        planned_fill_notional: crate::economics::PlannedFillNotional,
    ) -> Result<crate::economics::ResolvedEdgeBasis, EconomicsUnavailable> {
        self.validate_snapshot(request).map_err(|_| {
            EconomicsUnavailable::ProviderQuoteUnavailable {
                source_id: self.config.source_id.clone(),
            }
        })?;
        Ok(crate::economics::ResolvedEdgeBasis {
            normalized_amount: crate::economics::EdgeBasisAmount::new(
                planned_fill_notional.amount(),
            )?,
            source_snapshot_ids: vec![SnapshotId::new(self.snapshot.snapshot_id.clone())?],
            valid_until_ns: self.snapshot.valid_until_ns,
        })
    }

    fn quote(
        &self,
        request: &EconomicQuoteRequest,
        _planned_fill_notional: crate::economics::PlannedFillNotional,
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
