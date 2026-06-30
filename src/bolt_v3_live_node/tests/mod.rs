#![cfg(test)]

use super::*;
use crate::bolt_v3_capital_reservation::ReservationRejectionReason;
use crate::bolt_v3_config::{
    BoltV3RootConfig, ClientBlock, DataClientReadinessProbeBlock,
    DataClientReadinessProbeMarketDataKind, DataClientReadinessProbeQuoteTargetBlock,
    DataClientReadinessProbeQuoteTargetSource, DataInstrumentBlock,
    RealizedVolatilityAggregationBlock, RealizedVolatilityPolicyBlock,
    RealizedVolatilitySampleKindBlock, RealizedVolatilitySourceBlock,
    RealizedVolatilitySourceClassBlock, RealizedVolatilitySurfaceBlock,
};
use crate::bolt_v3_iv::config::IvRootConfig;
use crate::bolt_v3_iv::error::IvRejectReason;
use crate::bolt_v3_loss_governor::{LossSnapshot, LossSourceObservationTimestamps};
use crate::bolt_v3_providers::hyperliquid::{
    ResolvedBoltV3HyperliquidSecrets, hyperliquid_live_submit_signer_fingerprint,
};
use crate::bolt_v3_providers::hyperliquid_artifacts::{
    HyperliquidLiveSubmitApprovalInput, HyperliquidLiveSubmitOrderLimits,
    HyperliquidProductSubmitProofBinding, write_hyperliquid_live_submit_approval_artifact,
};
use nautilus_core::{UUID4, UnixNanos};
use nautilus_model::data::{BookOrder, OrderBookDelta, OrderBookDeltas};
use nautilus_model::enums::{
    AccountType, BookAction, CurrencyType, OrderSide, TimeInForce, TradingState,
};
use nautilus_model::events::{AccountState, OrderAccepted, OrderEventAny, OrderSubmitted};
use nautilus_model::identifiers::{
    AccountId, ClientId, ClientOrderId, TraderId, Venue, VenueOrderId,
};
use nautilus_model::orders::{LimitOrder, MarketOrder, OrderAny};
use nautilus_model::types::{AccountBalance, Currency, Money, Price, Quantity};
use rust_decimal::Decimal;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_CATALOG_ID: AtomicU64 = AtomicU64::new(0);

mod data_client_probe;
mod fixtures;
mod iv_runtime;
mod live_node_config;
mod provider_approvals;
mod startup_rebuild;
mod strategy_free_probe;
mod transport_scope;

use fixtures::*;
