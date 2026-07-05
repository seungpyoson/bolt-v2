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
use crate::bolt_v3_operator_health::BoltV3OperatorHealthStatus;
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
mod governance_mode;
mod iv_runtime;
mod live_node_config;
mod provider_approvals;
mod startup_rebuild;
mod strategy_free_probe;
mod transport_scope;

use fixtures::*;

#[test]
fn live_operator_health_surface_renders_poisoned_reject_feed_as_degraded() {
    let writer = Arc::new(NoStrategyDecisionEvidenceWriter);
    let feed = Arc::new(Mutex::new(BoltV3OrderRejectObserverFeed::new(
        writer.clone(),
        AccountId::from("ACCOUNT-POISON"),
    )));
    let poison_feed = feed.clone();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _guard = poison_feed
            .lock()
            .expect("test should acquire feed lock before poisoning it");
        panic!("poison reject feed");
    }));
    let submit_admission = BoltV3SubmitAdmissionState::new(writer);

    let surface = live_operator_health_surface(Some(&feed), &submit_admission, false, 0, None);

    assert_eq!(
        surface.reject_observer.status,
        BoltV3OperatorHealthStatus::Degraded
    );
    assert_eq!(
        surface.reject_observer.read_error.as_deref(),
        Some(OPERATOR_HEALTH_REJECT_OBSERVER_READ_ERROR)
    );
}

#[test]
fn shutdown_classifier_surfaces_drain_failure_after_clean_run_and_capture() {
    let error = classify_live_node_shutdown(
        Ok(()),
        Ok(()),
        Err(BoltV3LiveNodeError::DecisionEvidenceShutdownDrain(
            anyhow::anyhow!("drain failed"),
        )),
    )
    .expect_err("drain failure must be surfaced");

    assert!(matches!(
        error,
        BoltV3LiveNodeError::DecisionEvidenceShutdownDrain(_)
    ));
}

#[test]
fn shutdown_classifier_composes_runtime_failure_with_drain_failure() {
    let error = classify_live_node_shutdown(
        Err(BoltV3LiveNodeError::RuntimeCaptureShutdown(
            anyhow::anyhow!("capture failed"),
        )),
        Ok(()),
        Err(BoltV3LiveNodeError::DecisionEvidenceShutdownDrain(
            anyhow::anyhow!("drain failed"),
        )),
    )
    .expect_err("runtime and drain failures must compose");

    assert!(matches!(
        error,
        BoltV3LiveNodeError::RunAndDecisionEvidenceShutdownDrain { .. }
    ));
    assert!(error.to_string().contains(
        "LiveNode run, runtime-capture, or IV lifecycle stop failed and bolt-v3 decision evidence shutdown drain failed"
    ));
}

#[test]
fn shutdown_classifier_composes_iv_stop_failure_with_drain_failure() {
    let error = classify_live_node_shutdown(
        Ok(()),
        Err(BoltV3LiveNodeError::Run(anyhow::anyhow!(
            "iv lifecycle stop failed"
        ))),
        Err(BoltV3LiveNodeError::DecisionEvidenceShutdownDrain(
            anyhow::anyhow!("drain failed"),
        )),
    )
    .expect_err("IV stop and drain failures must compose");

    assert!(matches!(
        error,
        BoltV3LiveNodeError::RunAndDecisionEvidenceShutdownDrain { .. }
    ));
    assert!(error.to_string().contains(
        "LiveNode run, runtime-capture, or IV lifecycle stop failed and bolt-v3 decision evidence shutdown drain failed"
    ));
}
