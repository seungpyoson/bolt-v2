use std::collections::BTreeMap;

use anyhow::{Context, Result};
use nautilus_core::UnixNanos;
use nautilus_model::{
    enums::OrderSide,
    events::OrderEventAny,
    identifiers::{AccountId, InstrumentId, VenueOrderId},
    types::{Currency, Money},
};
use nautilus_polymarket::{
    common::{
        consts::{POLYMARKET, USDC_DECIMALS},
        credential::Secrets as PolymarketSecrets,
        enums::PolymarketOrderSide,
    },
    http::{
        clob::PolymarketClobHttpClient,
        data_api::PolymarketDataApiHttpClient,
        models::{DataApiPosition, PolymarketOpenOrder},
        query::BalanceAllowance,
        query::{AssetType, GetBalanceAllowanceParams, GetOrdersParams},
    },
};
use rust_decimal::Decimal;

use crate::bolt_v3_prediction_market_instrument::prediction_market_product_id_from_instrument_id;
use crate::bolt_v3_venue_truth::{
    VenueTruthCaptureEndpoint, VenueTruthCaptureEndpointError, VenueTruthCaptureErrorClass,
    VenueTruthOpenOrder, VenueTruthOrderEvent, VenueTruthOrderEventMapper,
    VenueTruthOrderEventTimestampDomain, VenueTruthSnapshot, VenueTruthSnapshotFuture,
    VenueTruthSnapshotSource,
};

use super::{PolymarketExecutionConfig, ResolvedBoltV3PolymarketSecrets};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolymarketVenueTruthRuntimeSourceConfig {
    pub poll_interval_ms: u64,
}

#[derive(Debug, Clone)]
pub struct PolymarketVenueTruthInput {
    pub captured_at: UnixNanos,
    pub account_id: AccountId,
    pub collateral_currency: Currency,
    pub collateral: BalanceAllowance,
    pub open_orders: Vec<PolymarketOpenOrder>,
    pub positions: Vec<DataApiPosition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolymarketVenueTruthBuildError {
    MissingCollateralAllowance,
    InvalidCollateralMoney,
    InvalidOpenOrderQuantity { venue_order_id: VenueOrderId },
    InvalidPositionSize { token_id: String },
}

#[derive(Debug, Clone, Copy)]
pub struct PolymarketVenueTruthOrderEventMapper;

#[derive(Clone)]
pub struct PolymarketVenueTruthRuntimeSource {
    account_id: AccountId,
    collateral_currency: Currency,
    clob_client: PolymarketClobHttpClient,
    data_api_client: PolymarketDataApiHttpClient,
    balance_allowance_params: GetBalanceAllowanceParams,
    user_address: String,
}

impl std::fmt::Debug for PolymarketVenueTruthRuntimeSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolymarketVenueTruthRuntimeSource")
            .field("account_id", &self.account_id)
            .field("collateral_currency", &self.collateral_currency)
            .finish_non_exhaustive()
    }
}

pub fn build_polymarket_venue_truth_runtime_source(
    cfg: &PolymarketExecutionConfig,
    resolved: &ResolvedBoltV3PolymarketSecrets,
    collateral_currency: Currency,
) -> Result<PolymarketVenueTruthRuntimeSource> {
    let polymarket_secrets = PolymarketSecrets::resolve(
        Some(resolved.private_key.as_str()),
        Some(resolved.api_key.as_str().to_owned()),
        Some(resolved.api_secret.as_str().to_owned()),
        Some(resolved.passphrase.as_str().to_owned()),
        cfg.funder.clone(),
    )
    .context("resolve Polymarket credentials for venue-truth runtime source")?;
    let user_address = polymarket_secrets
        .funder
        .clone()
        .unwrap_or_else(|| polymarket_secrets.address.clone());
    let clob_client = PolymarketClobHttpClient::new(
        polymarket_secrets.credential,
        polymarket_secrets.address,
        Some(cfg.base_url_http.clone()),
        cfg.http_timeout_secs,
    )
    .context("construct Polymarket CLOB venue-truth client")?;
    let data_api_client = PolymarketDataApiHttpClient::new(
        Some(cfg.base_url_data_api.clone()),
        cfg.http_timeout_secs,
    )
    .context("construct Polymarket Data API venue-truth client")?;

    Ok(PolymarketVenueTruthRuntimeSource {
        account_id: cfg.account_id,
        collateral_currency,
        clob_client,
        data_api_client,
        balance_allowance_params: GetBalanceAllowanceParams {
            asset_type: Some(AssetType::Collateral),
            token_id: None,
            signature_type: Some(super::nt_signature_type(cfg.signature_type)),
        },
        user_address,
    })
}

impl PolymarketVenueTruthRuntimeSource {
    async fn snapshot_inner(&self, captured_at: UnixNanos) -> Result<VenueTruthSnapshot> {
        let (collateral, open_orders, positions) = tokio::try_join!(
            async {
                self.clob_client
                    .get_balance_allowance(self.balance_allowance_params.clone())
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!(VenueTruthCaptureEndpointError::new(
                            VenueTruthCaptureEndpoint::ClobBalanceAllowance,
                            VenueTruthCaptureErrorClass::TransportOrDecode,
                            error.into(),
                        ))
                    })
            },
            async {
                self.clob_client
                    .get_orders(GetOrdersParams {
                        id: None,
                        market: None,
                        asset_id: None,
                        next_cursor: None,
                    })
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!(VenueTruthCaptureEndpointError::new(
                            VenueTruthCaptureEndpoint::ClobOpenOrders,
                            VenueTruthCaptureErrorClass::TransportOrDecode,
                            error.into(),
                        ))
                    })
            },
            async {
                self.data_api_client
                    .get_positions(&self.user_address)
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!(VenueTruthCaptureEndpointError::new(
                            VenueTruthCaptureEndpoint::DataApiPositions,
                            VenueTruthCaptureErrorClass::TransportOrDecode,
                            error.into(),
                        ))
                    })
            },
        )
        .context("poll Polymarket venue-truth endpoints")?;

        build_polymarket_venue_truth_snapshot(PolymarketVenueTruthInput {
            captured_at,
            account_id: self.account_id,
            collateral_currency: self.collateral_currency,
            collateral,
            open_orders,
            positions,
        })
        .map_err(|error| anyhow::anyhow!("convert Polymarket venue-truth snapshot: {error:?}"))
    }
}

impl VenueTruthSnapshotSource for PolymarketVenueTruthRuntimeSource {
    fn snapshot(&self, captured_at: UnixNanos) -> VenueTruthSnapshotFuture<'_> {
        Box::pin(async move { self.snapshot_inner(captured_at).await })
    }
}

impl VenueTruthOrderEventMapper for PolymarketVenueTruthOrderEventMapper {
    fn map_order_event(&self, event: &OrderEventAny) -> Option<VenueTruthOrderEvent> {
        if event.instrument_id().venue.as_str() != POLYMARKET {
            return None;
        }
        match event {
            OrderEventAny::Accepted(accepted) => Some(VenueTruthOrderEvent::Accepted {
                client_order_id: accepted.client_order_id.to_string(),
                venue_order_id: accepted.venue_order_id,
                observed_at_ns: accepted.ts_event,
            }),
            OrderEventAny::Filled(fill) => {
                let product_id = extract_polymarket_token_id(&fill.instrument_id)?;
                Some(VenueTruthOrderEvent::Filled {
                    venue_order_id: fill.venue_order_id,
                    product_id,
                    side: fill.order_side,
                    quantity: fill.last_qty.as_decimal(),
                    fill_price: fill.last_px.as_decimal(),
                    fee: fill
                        .commission
                        .as_ref()
                        .map_or(Decimal::ZERO, Money::as_decimal),
                    observed_at_ns: fill.ts_event,
                })
            }
            OrderEventAny::Canceled(_) | OrderEventAny::Expired(_) | OrderEventAny::Rejected(_) => {
                Some(VenueTruthOrderEvent::Terminal {
                    client_order_id: event.client_order_id().to_string(),
                    venue_order_id: event.venue_order_id(),
                    observed_at_ns: event.ts_event(),
                    timestamp_domain: VenueTruthOrderEventTimestampDomain::Venue,
                })
            }
            OrderEventAny::Denied(_) => Some(VenueTruthOrderEvent::Terminal {
                client_order_id: event.client_order_id().to_string(),
                venue_order_id: event.venue_order_id(),
                observed_at_ns: event.ts_event(),
                timestamp_domain: VenueTruthOrderEventTimestampDomain::Local,
            }),
            _ => None,
        }
    }
}

pub fn extract_polymarket_token_id(instrument_id: &InstrumentId) -> Option<String> {
    if instrument_id.venue.as_str() != POLYMARKET {
        return None;
    }
    prediction_market_product_id_from_instrument_id(instrument_id)
}

pub fn build_polymarket_venue_truth_snapshot(
    input: PolymarketVenueTruthInput,
) -> Result<VenueTruthSnapshot, PolymarketVenueTruthBuildError> {
    let collateral_balance =
        money_from_clob_pusd_units(input.collateral.balance, input.collateral_currency)?;
    let collateral_allowance = money_from_clob_pusd_units(
        input
            .collateral
            .allowance
            .ok_or(PolymarketVenueTruthBuildError::MissingCollateralAllowance)?,
        input.collateral_currency,
    )?;

    let mut open_orders = BTreeMap::new();
    for order in input.open_orders {
        let venue_order_id = VenueOrderId::from(order.id.as_str());
        let open_size = order.original_size - order.size_matched;
        if open_size < Decimal::ZERO {
            return Err(PolymarketVenueTruthBuildError::InvalidOpenOrderQuantity {
                venue_order_id,
            });
        }
        open_orders.insert(
            venue_order_id,
            VenueTruthOpenOrder {
                venue_order_id,
                market_id: order.market.to_string(),
                product_id: order.asset_id.to_string(),
                side: order_side(order.side),
                original_size: order.original_size,
                size_matched: order.size_matched,
                open_size,
                price: order.price,
            },
        );
    }

    let mut positions_by_product_id = BTreeMap::new();
    for position in input.positions {
        let size = position.size;
        if size < Decimal::ZERO {
            return Err(PolymarketVenueTruthBuildError::InvalidPositionSize {
                token_id: position.asset,
            });
        }
        positions_by_product_id
            .entry(position.asset)
            .and_modify(|current| *current += size)
            .or_insert(size);
    }

    Ok(VenueTruthSnapshot {
        captured_at: input.captured_at,
        account_id: input.account_id,
        collateral_balance,
        collateral_allowance,
        open_orders,
        positions_by_product_id,
    })
}

fn money_from_clob_pusd_units(
    amount: Decimal,
    currency: Currency,
) -> Result<Money, PolymarketVenueTruthBuildError> {
    Money::from_decimal(amount * clob_pusd_unit_scale(), currency)
        .map_err(|_| PolymarketVenueTruthBuildError::InvalidCollateralMoney)
}

fn clob_pusd_unit_scale() -> Decimal {
    Decimal::new(1, USDC_DECIMALS)
}

fn order_side(side: PolymarketOrderSide) -> OrderSide {
    match side {
        PolymarketOrderSide::Buy => OrderSide::Buy,
        PolymarketOrderSide::Sell => OrderSide::Sell,
    }
}
