//! Polymarket collateral allowance input for Bolt admission policy.

use anyhow::{Context, Result};
use nautilus_core::UnixNanos;
use nautilus_model::{identifiers::AccountId, types::Currency};
use nautilus_polymarket::{
    common::{
        consts::{POLYMARKET, USDC_DECIMALS},
        credential::Secrets as PolymarketSecrets,
    },
    http::{
        clob::PolymarketClobHttpClient,
        query::BalanceAllowance,
        query::{AssetType, GetBalanceAllowanceParams},
    },
};
use rust_decimal::Decimal;

use crate::{
    bolt_v3_capital_admission_state::{
        POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE, ProviderCollateralAllowanceSnapshot,
    },
    bolt_v3_provider_collateral_allowance::{
        ProviderCollateralAllowanceCaptureEndpoint,
        ProviderCollateralAllowanceCaptureEndpointError,
        ProviderCollateralAllowanceCaptureErrorClass, ProviderCollateralAllowanceSnapshotFuture,
        ProviderCollateralAllowanceSnapshotSource,
    },
};

use super::{PolymarketExecutionConfig, ResolvedBoltV3PolymarketSecrets};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketProviderCollateralAllowanceRuntimeSourceConfig {
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone)]
pub struct PolymarketProviderCollateralAllowanceInput {
    pub captured_at: UnixNanos,
    pub account_id: AccountId,
    pub collateral_currency: Currency,
    pub collateral: BalanceAllowance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolymarketProviderCollateralAllowanceBuildError {
    MissingCollateralAllowance,
    InvalidCollateralMoney,
}

#[derive(Clone)]
pub struct PolymarketProviderCollateralAllowanceRuntimeSource {
    account_id: AccountId,
    collateral_currency: Currency,
    clob_client: PolymarketClobHttpClient,
    balance_allowance_params: GetBalanceAllowanceParams,
}

impl std::fmt::Debug for PolymarketProviderCollateralAllowanceRuntimeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolymarketProviderCollateralAllowanceRuntimeSource")
            .field("account_id", &self.account_id)
            .field("collateral_currency", &self.collateral_currency)
            .finish_non_exhaustive()
    }
}

pub fn build_polymarket_provider_collateral_allowance_runtime_source(
    cfg: &PolymarketExecutionConfig,
    resolved: &ResolvedBoltV3PolymarketSecrets,
    collateral_currency: Currency,
) -> Result<PolymarketProviderCollateralAllowanceRuntimeSource> {
    let polymarket_secrets = PolymarketSecrets::resolve(
        Some(resolved.private_key.as_str()),
        Some(resolved.api_key.as_str().to_owned()),
        Some(resolved.api_secret.as_str().to_owned()),
        Some(resolved.passphrase.as_str().to_owned()),
        cfg.funder.clone(),
    )
    .context("resolve Polymarket credentials for provider-allowance runtime source")?;
    let clob_client = PolymarketClobHttpClient::new(
        polymarket_secrets.credential,
        polymarket_secrets.address,
        Some(cfg.base_url_http.clone()),
        cfg.http_timeout_secs,
    )
    .context("construct Polymarket CLOB provider-allowance client")?;
    Ok(PolymarketProviderCollateralAllowanceRuntimeSource {
        account_id: cfg.account_id,
        collateral_currency,
        clob_client,
        balance_allowance_params: GetBalanceAllowanceParams {
            asset_type: Some(AssetType::Collateral),
            token_id: None,
            signature_type: Some(super::nt_signature_type(cfg.signature_type)),
        },
    })
}

impl PolymarketProviderCollateralAllowanceRuntimeSource {
    async fn snapshot_inner(
        &self,
        captured_at: UnixNanos,
    ) -> Result<ProviderCollateralAllowanceSnapshot> {
        let collateral = self
            .clob_client
            .get_balance_allowance(self.balance_allowance_params.clone())
            .await
            .map_err(|error| {
                anyhow::anyhow!(ProviderCollateralAllowanceCaptureEndpointError::new(
                    ProviderCollateralAllowanceCaptureEndpoint::ClobBalanceAllowance,
                    ProviderCollateralAllowanceCaptureErrorClass::TransportOrDecode,
                    error.into(),
                ))
            })
            .context("poll Polymarket provider collateral allowance")?;

        build_polymarket_provider_collateral_allowance_snapshot(
            PolymarketProviderCollateralAllowanceInput {
                captured_at,
                account_id: self.account_id,
                collateral_currency: self.collateral_currency,
                collateral,
            },
        )
        .map_err(|error| {
            anyhow::anyhow!("convert Polymarket provider-allowance snapshot: {error:?}")
        })
    }
}

impl ProviderCollateralAllowanceSnapshotSource
    for PolymarketProviderCollateralAllowanceRuntimeSource
{
    fn snapshot(&self, captured_at: UnixNanos) -> ProviderCollateralAllowanceSnapshotFuture<'_> {
        Box::pin(async move { self.snapshot_inner(captured_at).await })
    }
}

pub fn build_polymarket_provider_collateral_allowance_snapshot(
    input: PolymarketProviderCollateralAllowanceInput,
) -> Result<ProviderCollateralAllowanceSnapshot, PolymarketProviderCollateralAllowanceBuildError> {
    let collateral_allowance = decimal_from_clob_pusd_units(
        input
            .collateral
            .allowance
            .ok_or(PolymarketProviderCollateralAllowanceBuildError::MissingCollateralAllowance)?,
        input.collateral_currency,
    )?;

    Ok(ProviderCollateralAllowanceSnapshot {
        source: POLYMARKET_PROVIDER_COLLATERAL_ALLOWANCE_REST_SOURCE.to_string(),
        observed_at_ns: input.captured_at.as_u64(),
        venue_id: POLYMARKET.to_string(),
        account_id: input.account_id.to_string(),
        collateral_currency: input.collateral_currency.to_string(),
        collateral_allowance,
    })
}

fn decimal_from_clob_pusd_units(
    amount: Decimal,
    currency: Currency,
) -> Result<Decimal, PolymarketProviderCollateralAllowanceBuildError> {
    nautilus_model::types::Money::from_decimal(amount * clob_pusd_unit_scale(), currency)
        .map(|money| money.as_decimal())
        .map_err(|_| PolymarketProviderCollateralAllowanceBuildError::InvalidCollateralMoney)
}

fn clob_pusd_unit_scale() -> Decimal {
    Decimal::new(1, USDC_DECIMALS)
}
