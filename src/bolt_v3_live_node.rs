//! Bolt-v3 NautilusTrader LiveNode assembly without strategy registration,
//! market selection, order construction, or ordinary strategy submit paths.
//!
//! Bolt-v3 LiveNode controlled-build / controlled-connect /
//! controlled-disconnect boundary. This module:
//!
//! - validates the forbidden credential env-var blocklist before
//!   constructing any NautilusTrader client
//! - resolves SSM secrets via the bolt-v3 secret resolver
//! - maps the validated bolt-v3 client blocks into provider-owned
//!   NT-native adapter configs
//! - registers the per-client NT data and execution client factories on a
//!   `nautilus_live::builder::LiveNodeBuilder` via the
//!   [`crate::bolt_v3_client_registration`] boundary
//! - calls `LiveNodeBuilder::build`, which is **not** purely passive:
//!   it constructs the NT client objects, lets provider-owned NT
//!   factories parse their credential material, and performs internal
//!   NT engine/message-bus subscriptions for venue instrument topics.
//!   None of these steps open a network connection or run the event
//!   loop.
//! - returns the resulting `nautilus_live::node::LiveNode` to the caller
//!   without entering the NT runner loop from the build path
//! - wires the existing `crate::nt_runtime_capture` from the
//!   `[persistence]` / `[persistence.streaming]` blocks
//! - permits only the kill-switch forced-reduction flatten effect to hand an
//!   already-admitted order to NT risk execution; ordinary strategy order
//!   construction and policy stay outside this module
//! - installs module-level logger filters from provider-owned bindings
//!   that suppress NT credential info logs even when the root TOML log
//!   level is `INFO`
//!
//! The caller owns the `LiveNode`; the build path never opens an
//! external network connection. Opt-in controlled-connect/strategy-free
//! readiness boundaries may open adapter sockets. The production
//! trading runner entrypoint is [`run_bolt_v3_live_node`]. The strategy-free
//! readiness path builds a strategy-free node before using NT's supported
//! runner loop with handle-driven stop; its dedicated quote probes call
//! only NT quote subscribe/unsubscribe APIs for client-owned readiness-probe
//! instruments. This module still never constructs an order or enables any
//! submit path from its own boundary code.

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, HashMap},
    future::Future,
    path::Path,
    pin::Pin,
    rc::Rc,
    str::FromStr,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ahash::AHashMap;
use anyhow::Result;
use log::LevelFilter;
use nautilus_common::{
    cache::Cache,
    component::Component,
    enums::Environment,
    logging::logger::LoggerConfig,
    messages::{
        SubscribeCommand, UnsubscribeCommand,
        data::{
            DataCommand,
            subscribe::{
                SubscribeBookDeltas, SubscribeCustomData, SubscribeOptionChain,
                SubscribeOptionGreeks, SubscribeQuotes, SubscribeTrades,
            },
            unsubscribe::{
                UnsubscribeBookDeltas, UnsubscribeCustomData, UnsubscribeOptionChain,
                UnsubscribeOptionGreeks, UnsubscribeQuotes, UnsubscribeTrades,
            },
        },
    },
    msgbus::{
        self, MStr, Pattern, ShareableMessageHandler, TypedHandler, subscribe_position_events,
        switchboard, unsubscribe_position_events,
    },
    runner::get_data_cmd_sender,
};
use nautilus_core::{Params, UUID4, time::get_atomic_clock_realtime};
use nautilus_live::{
    builder::LiveNodeBuilder,
    config::LiveNodeConfig,
    node::{LiveNode, LiveNodeHandle, NodeState},
};
use nautilus_model::{
    data::{
        CustomData, DataType, OptionChainSlice, OptionGreeks, OrderBookDeltas, QuoteTick,
        TradeTick, option_chain::StrikeRange,
    },
    enums::{BarIntervalType, BookType},
    identifiers::{AccountId, ClientId, InstrumentId, OptionSeriesId, StrategyId, Venue},
    instruments::{Instrument, InstrumentAny},
    types::Price,
};
#[cfg(test)]
use nautilus_model::{enums::AggressorSide, identifiers::TradeId};
use nautilus_model::{
    enums::{OrderSide, OrderType, TradingState},
    events::PositionEvent,
    orders::{Order, OrderAny},
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ustr::Ustr;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoltV3LoggingNotReadyForRun {
    max_level: LevelFilter,
    error_enabled: bool,
}

impl std::fmt::Display for BoltV3LoggingNotReadyForRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "bolt-v3 logging is not initialized before node run: max_level={:?}, error_enabled={}",
            self.max_level, self.error_enabled
        )
    }
}

impl std::error::Error for BoltV3LoggingNotReadyForRun {}

pub fn assert_bolt_v3_logging_ready_for_run() -> Result<(), BoltV3LoggingNotReadyForRun> {
    let max_level = log::max_level();
    let error_enabled = log::log_enabled!(log::Level::Error);
    if max_level == LevelFilter::Off || !error_enabled {
        return Err(BoltV3LoggingNotReadyForRun {
            max_level,
            error_enabled,
        });
    }
    Ok(())
}

#[cfg(test)]
use crate::bolt_v3_current_evidence::DecisionEvidenceRecorder;
use crate::{
    bolt_v3_adapters::{
        BoltV3AdapterConfigs, BoltV3AdapterMappingError, map_bolt_v3_adapters,
        map_bolt_v3_adapters_with_runtime_approvals,
    },
    bolt_v3_capital_admission::{
        CapitalAdmissionPolicy, FeeSlippagePolicy, PredictionMarketAdmissionSnapshot,
        ProductAdmissionSnapshot, ProductKind,
    },
    bolt_v3_capital_admission_runtime_feed::{
        CapitalAdmissionNtCacheProjection, CapitalAdmissionRuntimeFeed,
        CapitalAdmissionRuntimeFeedConfig, SubmitAdmissionNtProjectionSubscription,
        subscribe_submit_admission_nt_projection,
    },
    bolt_v3_capital_admission_state::ProviderCollateralAllowanceSnapshot,
    bolt_v3_capital_reservation::CapitalPoolSnapshot,
    bolt_v3_client_registration::{
        BoltV3ClientRegistrationError, BoltV3RegistrationSummary, register_bolt_v3_clients,
    },
    bolt_v3_config::{
        BoltV3RootConfig, CapitalPoolBlock, ClientBlock, DataClientReadinessProbeBlock,
        DataClientReadinessProbeBookType, DataClientReadinessProbeMarketDataKind,
        DataClientReadinessProbeQuoteTargetSource, LiveSubmitGovernanceMode, LoadedBoltV3Config,
        LoadedStrategy, nautilus_startup_bound_secs,
    },
    bolt_v3_current_evidence::{
        CapitalAdmissionRebuildSource, DecisionEvidenceRuntime, DecisionEvidenceStatusView,
        ObservationStreamStatus, ReservationRecoveryFacts,
    },
    bolt_v3_iv::{
        config::IvRootConfig,
        health::IvSourceHealth,
        runtime::{
            IvRuntimeBindingAdapter, IvRuntimeBindingError, IvRuntimeEngine,
            apply_subscription_plans,
        },
        selector::IvSelector,
        subscription::{
            IvRuntimeOperation, IvSubscriptionError, IvSubscriptionPlan, plan_profile_reload,
            plan_profile_start, plan_profile_stop,
        },
        time::UnixNanos,
        types::IvSourceKind,
    },
    bolt_v3_kill_switch::{
        KillSwitchEvent, KillSwitchHaltTrigger, KillSwitchState, KillSwitchStateKind,
        KillSwitchTransitionContext, transition_kill_switch_state,
    },
    bolt_v3_kill_switch_store::{
        KillSwitchRecoveryReason, KillSwitchRecoveryState, KillSwitchStore, KillSwitchStoreError,
    },
    bolt_v3_loss_governor::{
        LossGovernorPolicy, evaluate_loss_admission, evaluate_loss_admission_with_observations,
    },
    bolt_v3_loss_halt_actions::{
        LossGovernorHaltActionHandler, LossGovernorHaltActionPolicy,
        LossGovernorManualRecoveryEvidence, LossGovernorManualRecoveryRequest,
        LossGovernorRecoveryMode, LossGovernorTradingStateAction,
        next_loss_governor_manual_recovery_trading_state, next_loss_governor_trading_state,
    },
    bolt_v3_loss_protection::{
        KillSwitchLossAction, KillSwitchLossActionKind, KillSwitchLossActionSink,
        KillSwitchLossProtection, KillSwitchLossProtectionConfig, PositionRealizedPnlObservation,
    },
    bolt_v3_loss_runtime_feed::{
        LossGovernorRuntimeFeed, LossGovernorRuntimeFeedConfig,
        LossGovernorRuntimeFeedSubscription, subscribe_loss_governor_runtime_feed,
    },
    bolt_v3_operator_health::{
        BoltV3DecisionEvidenceObservationHealth, BoltV3InputHealth,
        BoltV3InputHealthSourceTransition, BoltV3InputHealthTransitionEmitter,
        BoltV3MissingInputSource, BoltV3OperatorHealthSurface,
        BoltV3OperatorHealthTransitionEmitter, BoltV3ProviderCollateralAllowanceHealth,
        BoltV3RejectObserverHealth, BoltV3SettlementHealth, BoltV3SettlementHealthTransition,
        BoltV3SettlementHealthTransitionEmitter, node_scoped_runtime_source_announcements,
        runtime_source_announcements,
    },
    bolt_v3_order_reject_observer_feed::{
        BoltV3OrderRejectObserverFeed, OrderRejectObserverFeedSubscription,
        subscribe_order_reject_observer_feed_with_health_emitter,
    },
    bolt_v3_providers::{
        self, ProviderLiveSubmitApprovalContext, ProviderLiveSubmitApprovals,
        ProviderRuntimeApprovals, ReferencePriceIdentifierKind, reference_price_provider_metadata,
    },
    bolt_v3_reference_price::reference_price_source_is_runtime_available,
    bolt_v3_secrets::{
        BoltV3SecretError, ForbiddenEnvVarError, ResolvedBoltV3Secrets,
        check_no_forbidden_credential_env_vars, check_no_forbidden_credential_env_vars_with,
        resolve_bolt_v3_secrets, resolve_bolt_v3_secrets_with,
    },
    bolt_v3_settlement_runtime::{
        BoltV3SettlementRuntimeSink, BoltV3SettlementRuntimeSinkBackends,
        BoltV3SettlementRuntimeSinkHandle,
    },
    bolt_v3_strategy_registration::{
        BoltV3StrategyExecutionControls, BoltV3StrategyRegistrationError,
        register_bolt_v3_strategies_on_node_with_bindings,
        register_bolt_v3_strategies_on_node_with_iv_runtime_bindings,
    },
    bolt_v3_submit_admission::{
        BoltV3CompiledOrderSide, BoltV3LiveSubmitApprovalLimits, BoltV3SubmitAdmissionState,
        BoltV3SubmitCapitalAdmissionConfig,
        BoltV3SubmitCapitalAdmissionMissingNtAccountCacheBalance,
        BoltV3SubmitCapitalAdmissionOpenOrderEvidence,
        BoltV3SubmitCapitalAdmissionOpenOrderSnapshot, BoltV3SubmitCapitalAdmissionRebuildDecision,
    },
    bolt_v3_validate::parse_decimal_string,
    nt_runtime_capture::{
        NtRuntimeCaptureGuards, position_events_pattern, wire_nt_runtime_capture,
    },
    secrets::SsmResolverSession,
};

mod data_client_probe;
mod iv;
mod live_node_config;
mod risk_admission_loss;
mod secrets_builders;
mod transport_scope;

#[cfg(test)]
use data_client_probe::data_client_census_report;
use data_client_probe::data_client_probe_loaded_config;
pub use data_client_probe::{run_bolt_v3_data_client_census, run_bolt_v3_data_client_probe};
pub use iv::{
    BoltV3IvRuntimeEventBindings, IvEngineLifecyclePlan, plan_iv_engine_lifecycle,
    plan_iv_engine_reload_lifecycle, wire_bolt_v3_iv_runtime_event_bindings,
};
use iv::{
    NtIvRuntimeBindingAdapter, NtIvRuntimeCommandSenderAdapter, NtIvRuntimePlanValidationAdapter,
};
#[cfg(test)]
use iv::{
    iv_runtime_data_commands_for_plan, parse_option_chain_series_ids,
    parse_option_greeks_instrument_ids,
};
#[cfg(test)]
use live_node_config::make_bolt_v3_live_node_builder_from_config;
pub use live_node_config::{
    connect_bolt_v3_clients, disconnect_bolt_v3_clients, make_bolt_v3_live_node_builder,
    make_live_node_config, wire_bolt_v3_runtime_capture,
};
use risk_admission_loss::{
    BoltV3LossProtectionRuntimeGuards, BoltV3ProviderCollateralAllowanceRuntimeGuard,
    capital_admission_config_from_loaded, capital_admission_runtime_feed_config_from_loaded,
    configure_bolt_v3_kill_switch_loss_protection, loss_governor_halt_action_handler_from_node,
    loss_governor_halt_action_policy_from_loaded, loss_governor_policy_from_loaded,
    loss_governor_runtime_feed_config_from_loaded, order_reject_observer_account_id_from_loaded,
    provider_collateral_allowance_runtime_config_from_loaded,
    recover_kill_switch_state_before_live_node_build, spawn_provider_collateral_allowance_runtime,
    sync_nt_trading_state_for_kill_switch, wire_bolt_v3_loss_protection_runtime,
};
#[cfg(test)]
pub(crate) use secrets_builders::build_bolt_v3_strategy_free_live_node_for_data_clients_with_summary;
pub use secrets_builders::{
    build_bolt_v3_live_node_with_resolved, build_bolt_v3_strategy_free_data_client_probe_live_node,
    build_bolt_v3_strategy_free_live_node, build_bolt_v3_strategy_free_live_node_for_data_clients,
    build_bolt_v3_strategy_free_live_node_with_resolved,
    build_bolt_v3_strategy_free_live_node_with_resolved_for_data_clients,
    build_bolt_v3_strategy_free_live_node_with_summary,
    check_bolt_v3_strategy_free_live_node_for_data_clients_forbidden_env_vars_with,
};
use secrets_builders::{
    current_unix_nanos, live_node_adapter_bundle_with_provider_live_submit_approvals,
};
#[cfg(test)]
use secrets_builders::{
    live_node_adapter_bundle_with_provider_approvals_at,
    load_provider_live_submit_approvals_for_live_node,
};
#[cfg(test)]
use transport_scope::trade_transport_client_keys;
use transport_scope::{
    RealizedVolatilityTransportScope, trade_transport_loaded_config,
    validate_trade_transport_execution_venue_cardinality,
};

pub fn current_build_head_sha() -> Option<&'static str> {
    crate::bolt_v3_operator_artifacts::current_build_head_sha()
}

const OPERATOR_HEALTH_REASON_LIVE_NODE_STARTUP: &str = stringify!(live_node_startup);
const OPERATOR_HEALTH_REASON_SUBMIT_ADMISSION_NT_PROJECTION: &str =
    stringify!(submit_admission_nt_projection);
const OPERATOR_HEALTH_REJECT_OBSERVER_READ_ERROR: &str =
    stringify!(order_reject_observer_feed_lock_poisoned);
const OPERATOR_HEALTH_SUBMIT_ADMISSION_READ_ERROR: &str =
    stringify!(submit_admission_state_lock_poisoned);
const OPERATOR_HEALTH_INPUT_SOURCE_UNOBSERVED_REASON: &str =
    "no live input-health transition observed for reference_current_price source";

pub struct BoltV3LiveNodeRuntime {
    node: LiveNode,
    registration_summary: BoltV3RegistrationSummary,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    loss_protection: Option<Rc<RefCell<KillSwitchLossProtection>>>,
    loss_halt_action_policy: Option<LossGovernorHaltActionPolicy>,
    loss_runtime_feed: Option<Rc<RefCell<LossGovernorRuntimeFeed>>>,
    loss_runtime_feed_subscription: Option<LossGovernorRuntimeFeedSubscription>,
    order_reject_observer_feed: Option<Arc<Mutex<BoltV3OrderRejectObserverFeed>>>,
    order_reject_observer_feed_subscription: Option<OrderRejectObserverFeedSubscription>,
    capital_admission_runtime_feed: Option<Arc<Mutex<CapitalAdmissionRuntimeFeed>>>,
    submit_admission_nt_projection_subscription: Option<SubmitAdmissionNtProjectionSubscription>,
    submit_admission_nt_projection_trigger: Option<Rc<dyn Fn()>>,
    submit_admission_nt_projection_requested: Option<Arc<AtomicBool>>,
    provider_collateral_allowance_runtime_guard:
        Option<BoltV3ProviderCollateralAllowanceRuntimeGuard>,
    submit_reservation_recovery: Arc<ReservationRecoveryFacts>,
    submit_admission_nt_reconciliation_account_ids: BTreeSet<AccountId>,
    iv_runtime: Option<IvRuntimeEngine>,
    iv_event_bindings: Option<BoltV3IvRuntimeEventBindings>,
    operator_health_transition_logger: BoltV3OperatorHealthTransitionLogger,
    input_health_configured_source_count: usize,
    settlement_health: Arc<Mutex<BoltV3SettlementHealth>>,
    decision_evidence_runtime: DecisionEvidenceRuntime,
    decision_evidence: DecisionEvidenceStatusView,
    redaction_values: Vec<Zeroizing<String>>,
}

#[derive(Debug)]
struct BoltV3LiveNodeAdapterBundle {
    configs: BoltV3AdapterConfigs,
    live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3StrategyFreeReferenceCacheEvidence {
    cached_instrument_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3DataClientCensusReport {
    pub client_key: String,
    pub cached_instrument_count: usize,
    pub cached_instrument_ids_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoltV3DataClientProbeReport {
    pub client_key: String,
    pub market_data_kind: String,
    pub required_observation_count: usize,
    pub observed_update_count: usize,
}

mod strategy_free_probe;

use strategy_free_probe::*;

impl BoltV3StrategyFreeReferenceCacheEvidence {
    pub fn cached_instrument_ids(&self) -> &[String] {
        &self.cached_instrument_ids
    }
}

type BoltV3LiveSettlementLossProtectionSlot =
    Rc<RefCell<Option<Rc<RefCell<KillSwitchLossProtection>>>>>;

#[derive(Clone)]
struct BoltV3LiveSettlementRuntimeSink {
    loss_protection: Option<BoltV3LiveSettlementLossProtectionSlot>,
}

impl std::fmt::Debug for BoltV3LiveSettlementRuntimeSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoltV3LiveSettlementRuntimeSink")
            .field(
                stringify!(loss_protection),
                &self
                    .loss_protection
                    .as_ref()
                    .is_some_and(|loss_protection| loss_protection.borrow().is_some()),
            )
            .finish()
    }
}

impl BoltV3SettlementRuntimeSink for BoltV3LiveSettlementRuntimeSink {
    fn record_loss_governor_position_realized_pnl(
        &self,
        observation: PositionRealizedPnlObservation,
    ) -> Result<()> {
        if let Some(loss_protection) = self.loss_protection.as_ref()
            && let Some(loss_protection) = loss_protection.borrow().as_ref()
        {
            loss_protection
                .borrow_mut()
                .record_position_realized_pnl(observation)?;
        }
        Ok(())
    }
}

fn settlement_runtime_sink_handle(
    loss_protection: Option<BoltV3LiveSettlementLossProtectionSlot>,
) -> Option<BoltV3SettlementRuntimeSinkHandle> {
    loss_protection.as_ref()?;
    let sink: BoltV3SettlementRuntimeSinkHandle =
        Rc::new(BoltV3LiveSettlementRuntimeSink { loss_protection });
    Some(sink)
}

struct BoltV3LiveNodeRuntimeFeeds {
    loss_protection: Option<Rc<RefCell<KillSwitchLossProtection>>>,
    loss_halt_action_policy: Option<LossGovernorHaltActionPolicy>,
    loss_runtime_feed: Option<Rc<RefCell<LossGovernorRuntimeFeed>>>,
    loss_runtime_feed_subscription: Option<LossGovernorRuntimeFeedSubscription>,
    order_reject_observer_feed: Option<Arc<Mutex<BoltV3OrderRejectObserverFeed>>>,
    order_reject_observer_feed_subscription: Option<OrderRejectObserverFeedSubscription>,
    capital_admission_runtime_feed: Option<Arc<Mutex<CapitalAdmissionRuntimeFeed>>>,
    submit_admission_nt_projection_subscription: Option<SubmitAdmissionNtProjectionSubscription>,
    submit_admission_nt_projection_trigger: Option<Rc<dyn Fn()>>,
    submit_admission_nt_projection_requested: Option<Arc<AtomicBool>>,
    provider_collateral_allowance_runtime_guard:
        Option<BoltV3ProviderCollateralAllowanceRuntimeGuard>,
    submit_reservation_recovery: Arc<ReservationRecoveryFacts>,
    submit_admission_nt_reconciliation_account_ids: BTreeSet<AccountId>,
}

#[derive(Clone)]
struct BoltV3OperatorHealthTransitionLogger {
    last_surface: Arc<Mutex<Option<BoltV3OperatorHealthSurface>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BoltV3OperatorHealthTransitionEmission {
    Emitted,
    Deduped,
    LoggerLockPoisoned,
}

struct BoltV3DecisionEvidenceProducerGuards {
    loss_protection_guards: BoltV3LossProtectionRuntimeGuards,
    loss_runtime_feed_subscription: Option<LossGovernorRuntimeFeedSubscription>,
    order_reject_observer_feed_subscription: Option<OrderRejectObserverFeedSubscription>,
    submit_admission_nt_projection_subscription: Option<SubmitAdmissionNtProjectionSubscription>,
    provider_collateral_allowance_runtime_guard:
        Option<BoltV3ProviderCollateralAllowanceRuntimeGuard>,
}

trait BoltV3DecisionEvidenceProducerStopper {
    fn stop_before_decision_evidence_drain(
        self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>;
}

impl BoltV3DecisionEvidenceProducerStopper for BoltV3DecisionEvidenceProducerGuards {
    fn stop_before_decision_evidence_drain(
        self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>> {
        Box::pin(async move {
            let Self {
                loss_protection_guards,
                loss_runtime_feed_subscription,
                order_reject_observer_feed_subscription,
                submit_admission_nt_projection_subscription,
                provider_collateral_allowance_runtime_guard,
            } = self;
            drop(loss_runtime_feed_subscription);
            drop(order_reject_observer_feed_subscription);
            drop(submit_admission_nt_projection_subscription);
            if let Some(guard) = provider_collateral_allowance_runtime_guard {
                guard.stop_and_join();
            }
            loss_protection_guards.stop_and_join().await;
        })
    }
}

async fn drain_after_stopping_decision_evidence_producers<P, D, E>(
    producer_guards: P,
    drain: D,
) -> std::result::Result<(), E>
where
    P: BoltV3DecisionEvidenceProducerStopper,
    D: FnOnce() -> std::result::Result<(), E>,
{
    producer_guards.stop_before_decision_evidence_drain().await;
    drain()
}

impl BoltV3OperatorHealthTransitionLogger {
    fn new() -> Self {
        Self {
            last_surface: Arc::new(Mutex::new(None)),
        }
    }

    fn emit_surface(
        &self,
        reason: &'static str,
        surface: BoltV3OperatorHealthSurface,
    ) -> BoltV3OperatorHealthTransitionEmission {
        let mut last_surface = match self.last_surface.lock() {
            Ok(last_surface) => last_surface,
            Err(_) => {
                log::error!(
                    "bolt-v3 operator health transition cache lock poisoned; rendering current health surface without dedupe: reason={} surface={}",
                    reason,
                    serde_json::to_string(&surface)
                        .unwrap_or_else(|error| format!("serialization_failed:{error}"))
                );
                return BoltV3OperatorHealthTransitionEmission::LoggerLockPoisoned;
            }
        };
        if last_surface.as_ref() == Some(&surface) {
            return BoltV3OperatorHealthTransitionEmission::Deduped;
        }
        log::warn!(
            "bolt-v3 operator health transition: reason={} surface={}",
            reason,
            serde_json::to_string(&surface)
                .unwrap_or_else(|error| format!("serialization_failed:{error}"))
        );
        *last_surface = Some(surface);
        BoltV3OperatorHealthTransitionEmission::Emitted
    }
}

fn live_operator_health_surface(
    order_reject_observer_feed: Option<&Arc<Mutex<BoltV3OrderRejectObserverFeed>>>,
    submit_admission: &BoltV3SubmitAdmissionState,
    provider_collateral_allowance_configured: bool,
    input_health_configured_source_count: usize,
    input_health: Option<BoltV3InputHealth>,
    settlement_health: BoltV3SettlementHealth,
    decision_evidence: &DecisionEvidenceStatusView,
) -> BoltV3OperatorHealthSurface {
    let reject_observer = order_reject_observer_feed.map_or_else(
        BoltV3RejectObserverHealth::not_configured,
        |feed| match feed.lock() {
            Ok(feed) => BoltV3RejectObserverHealth::from_snapshot(&feed.health_snapshot()),
            Err(_) => {
                BoltV3RejectObserverHealth::read_error(OPERATOR_HEALTH_REJECT_OBSERVER_READ_ERROR)
            }
        },
    );
    let provider_collateral_allowance = if provider_collateral_allowance_configured {
        match submit_admission.operator_health_snapshot() {
            Ok(snapshot) => {
                BoltV3ProviderCollateralAllowanceHealth::from_configured_kill_switch_and_capital_state(
                    &snapshot.kill_switch_state,
                    snapshot.capital_admission_state.as_ref(),
                )
            }
            Err(_) => BoltV3ProviderCollateralAllowanceHealth::read_error_without_snapshot(
                OPERATOR_HEALTH_SUBMIT_ADMISSION_READ_ERROR,
            ),
        }
    } else {
        BoltV3ProviderCollateralAllowanceHealth::not_configured()
    };
    BoltV3OperatorHealthSurface::from_live_parts(
        reject_observer,
        provider_collateral_allowance,
        input_health.unwrap_or_else(|| {
            // If no live transition emitter has produced a source observation yet,
            // keep the surface fail-closed as Unobserved for the configured sources.
            BoltV3InputHealth::unobserved(input_health_configured_source_count)
        }),
        settlement_health,
        match decision_evidence
            .machine_stream_status()
            .expect("live decision-evidence runtime must own the machine status view")
        {
            ObservationStreamStatus::Available => {
                BoltV3DecisionEvidenceObservationHealth::available()
            }
            ObservationStreamStatus::Poisoned { cause } => {
                BoltV3DecisionEvidenceObservationHealth::poisoned(cause.as_ref())
            }
        },
        match decision_evidence
            .observation_stream_status()
            .expect("live decision-evidence runtime must own the observation status view")
        {
            ObservationStreamStatus::Available => {
                BoltV3DecisionEvidenceObservationHealth::available()
            }
            ObservationStreamStatus::Poisoned { cause } => {
                BoltV3DecisionEvidenceObservationHealth::poisoned(cause.as_ref())
            }
        },
    )
}

fn settlement_health_snapshot(
    settlement_health: &Mutex<BoltV3SettlementHealth>,
) -> Result<BoltV3SettlementHealth> {
    settlement_health
        .lock()
        .map_err(|_| anyhow::anyhow!("settlement health lock poisoned"))
        .map(|health| health.clone())
}

fn configured_reference_current_price_source_count(
    sources_by_client: &BTreeMap<String, Vec<BoltV3MissingInputSource>>,
) -> usize {
    sources_by_client.values().map(Vec::len).sum()
}

fn settlement_health_from_loaded(loaded: &LoadedBoltV3Config) -> BoltV3SettlementHealth {
    if loaded.strategies.iter().any(|strategy| {
        crate::strategy_bindings::production_runtime_bindings()
            .iter()
            .find(|binding| binding.key == strategy.config.strategy_archetype.as_str())
            .is_some_and(|binding| binding.capabilities.settlement)
    }) {
        BoltV3SettlementHealth::nominal()
    } else {
        BoltV3SettlementHealth::not_configured()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BoltV3InputHealthSourceKey {
    strategy_instance_id: String,
    source_id: String,
    asset: String,
    provider: String,
    provider_instrument: String,
}

impl BoltV3InputHealthSourceKey {
    fn from_source(source: &BoltV3MissingInputSource) -> Self {
        Self {
            strategy_instance_id: source.strategy_instance_id.clone(),
            source_id: source.source_id.clone(),
            asset: source.asset.clone(),
            provider: source.provider.clone(),
            provider_instrument: source.provider_instrument.clone(),
        }
    }
}

#[derive(Debug)]
struct BoltV3LiveInputHealthAccumulator {
    configured_source_count: usize,
    configured_sources: BTreeMap<BoltV3InputHealthSourceKey, BoltV3MissingInputSource>,
    observed_sources: BTreeSet<BoltV3InputHealthSourceKey>,
    missing_sources: BTreeMap<BoltV3InputHealthSourceKey, BoltV3MissingInputSource>,
    has_transition: bool,
}

impl BoltV3LiveInputHealthAccumulator {
    fn new(
        configured_source_count: usize,
        sources_by_client: &BTreeMap<String, Vec<BoltV3MissingInputSource>>,
    ) -> Self {
        let configured_sources = sources_by_client
            .values()
            .flat_map(|sources| sources.iter())
            .map(|source| {
                let mut source = source.clone();
                source.reason = OPERATOR_HEALTH_INPUT_SOURCE_UNOBSERVED_REASON.to_string();
                (BoltV3InputHealthSourceKey::from_source(&source), source)
            })
            .collect();
        Self {
            configured_source_count,
            configured_sources,
            observed_sources: BTreeSet::new(),
            missing_sources: BTreeMap::new(),
            has_transition: false,
        }
    }

    fn apply_transition(
        &mut self,
        transition: BoltV3InputHealthSourceTransition,
    ) -> BoltV3InputHealth {
        self.has_transition = true;
        let key = BoltV3InputHealthSourceKey::from_source(&transition.source);
        if transition.missing {
            self.observed_sources.remove(&key);
            self.missing_sources.insert(key, transition.source);
        } else {
            self.observed_sources.insert(key.clone());
            self.missing_sources.remove(&key);
        }
        self.snapshot()
    }

    fn snapshot(&self) -> BoltV3InputHealth {
        if self.configured_source_count == 0 {
            return BoltV3InputHealth::not_configured();
        }
        if !self.has_transition {
            return BoltV3InputHealth::unobserved(self.configured_source_count);
        }
        let mut missing_sources = Vec::new();
        for (key, source) in &self.configured_sources {
            if let Some(missing_source) = self.missing_sources.get(key) {
                missing_sources.push(missing_source.clone());
            } else if !self.observed_sources.contains(key) {
                missing_sources.push(source.clone());
            }
        }
        for (key, source) in &self.missing_sources {
            if !self.configured_sources.contains_key(key) {
                missing_sources.push(source.clone());
            }
        }
        BoltV3InputHealth::from_live_missing_sources(self.configured_source_count, missing_sources)
    }
}

fn live_input_health_snapshot(
    input_health_accumulator: &Mutex<BoltV3LiveInputHealthAccumulator>,
) -> Option<BoltV3InputHealth> {
    input_health_accumulator
        .lock()
        .ok()
        .map(|accumulator| accumulator.snapshot())
}

fn apply_live_input_health_transition(
    input_health_accumulator: &Mutex<BoltV3LiveInputHealthAccumulator>,
    configured_source_count: usize,
    transition: BoltV3InputHealthSourceTransition,
) -> BoltV3InputHealth {
    match input_health_accumulator.lock() {
        Ok(mut accumulator) => accumulator.apply_transition(transition),
        Err(_) => BoltV3InputHealth::unobserved(configured_source_count),
    }
}

fn build_settlement_health_transition_emitter(
    settlement_health: Arc<Mutex<BoltV3SettlementHealth>>,
    input_health_accumulator: Arc<Mutex<BoltV3LiveInputHealthAccumulator>>,
    emit_operator_health_surface: Arc<
        dyn Fn(&'static str, Option<BoltV3InputHealth>) -> Result<()> + Send + Sync + 'static,
    >,
) -> BoltV3SettlementHealthTransitionEmitter {
    Arc::new(move |transition: BoltV3SettlementHealthTransition| {
        settlement_health
            .lock()
            .map_err(|_| anyhow::anyhow!("settlement health lock poisoned"))?
            .apply_transition(transition);
        let input_health = live_input_health_snapshot(&input_health_accumulator);
        emit_operator_health_surface(stringify!(settlement_booking_terminal), input_health)?;
        Ok(())
    })
}

fn reference_current_price_live_input_sources_by_client(
    loaded: &LoadedBoltV3Config,
) -> BTreeMap<String, Vec<BoltV3MissingInputSource>> {
    let mut by_client = BTreeMap::<String, Vec<BoltV3MissingInputSource>>::new();
    for strategy in &loaded.strategies {
        let Some(reference) = strategy.config.reference_current_price.as_ref() else {
            continue;
        };
        for source_id in &reference.source_order {
            let Some(source) = reference.sources.get(source_id) else {
                continue;
            };
            if !reference_price_source_is_runtime_available(reference, source) {
                continue;
            }
            if !bolt_v3_providers::reference_price_provider_emits_live_input_health(
                source.provider.as_str(),
            ) {
                continue;
            }
            let Some(metadata) = reference_price_provider_metadata(source.provider.as_str()) else {
                continue;
            };
            let provider_instrument = match metadata.identifier_kind {
                ReferencePriceIdentifierKind::InstrumentId => source.instrument_id.clone(),
                ReferencePriceIdentifierKind::Symbol => source.symbol.clone(),
            };
            let Some(provider_instrument) = provider_instrument else {
                continue;
            };
            let sources = match by_client.entry(source.client_id.to_string()) {
                std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::btree_map::Entry::Vacant(entry) => entry.insert(Vec::new()),
            };
            sources.push(BoltV3MissingInputSource {
                strategy_instance_id: strategy.config.strategy_instance_id.clone(),
                source_id: source_id.clone(),
                asset: reference.asset.clone(),
                provider: source.provider.as_str().to_string(),
                provider_instrument,
                reason: OPERATOR_HEALTH_INPUT_SOURCE_UNOBSERVED_REASON.to_string(),
            });
        }
    }
    by_client
}

struct BoltV3LiveNodeRuntimeComponents {
    iv_runtime: Option<IvRuntimeEngine>,
    iv_event_bindings: Option<BoltV3IvRuntimeEventBindings>,
    operator_health_transition_logger: BoltV3OperatorHealthTransitionLogger,
    input_health_configured_source_count: usize,
    settlement_health: Arc<Mutex<BoltV3SettlementHealth>>,
    decision_evidence_runtime: DecisionEvidenceRuntime,
    decision_evidence: DecisionEvidenceStatusView,
    redaction_values: Vec<Zeroizing<String>>,
}

impl BoltV3LiveNodeRuntime {
    fn new(
        node: LiveNode,
        registration_summary: BoltV3RegistrationSummary,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
        feeds: BoltV3LiveNodeRuntimeFeeds,
        runtime_components: BoltV3LiveNodeRuntimeComponents,
    ) -> Self {
        Self {
            node,
            registration_summary,
            submit_admission,
            loss_protection: feeds.loss_protection,
            loss_halt_action_policy: feeds.loss_halt_action_policy,
            loss_runtime_feed: feeds.loss_runtime_feed,
            loss_runtime_feed_subscription: feeds.loss_runtime_feed_subscription,
            order_reject_observer_feed: feeds.order_reject_observer_feed,
            order_reject_observer_feed_subscription: feeds.order_reject_observer_feed_subscription,
            capital_admission_runtime_feed: feeds.capital_admission_runtime_feed,
            submit_admission_nt_projection_subscription: feeds
                .submit_admission_nt_projection_subscription,
            submit_admission_nt_projection_trigger: feeds.submit_admission_nt_projection_trigger,
            submit_admission_nt_projection_requested: feeds
                .submit_admission_nt_projection_requested,
            provider_collateral_allowance_runtime_guard: feeds
                .provider_collateral_allowance_runtime_guard,
            submit_reservation_recovery: feeds.submit_reservation_recovery,
            submit_admission_nt_reconciliation_account_ids: feeds
                .submit_admission_nt_reconciliation_account_ids,
            iv_runtime: runtime_components.iv_runtime,
            iv_event_bindings: runtime_components.iv_event_bindings,
            operator_health_transition_logger: runtime_components.operator_health_transition_logger,
            input_health_configured_source_count: runtime_components
                .input_health_configured_source_count,
            settlement_health: runtime_components.settlement_health,
            decision_evidence_runtime: runtime_components.decision_evidence_runtime,
            decision_evidence: runtime_components.decision_evidence,
            redaction_values: runtime_components.redaction_values,
        }
    }

    pub fn registered_strategy_ids(&self) -> Vec<StrategyId> {
        self.node.kernel().trader().borrow().strategy_ids()
    }

    pub fn write_launch_identity(
        &self,
        catalog_directory: &Path,
        identity: &crate::bolt_v3_operator_artifacts::LaunchIdentity,
    ) -> Result<
        crate::bolt_v3_operator_artifacts::WrittenOperatorArtifact,
        crate::bolt_v3_operator_artifacts::BoltV3OperatorArtifactError,
    > {
        self.decision_evidence_runtime
            .write_launch_identity(catalog_directory, identity)
    }

    pub fn environment(&self) -> Environment {
        self.node.environment()
    }

    pub fn state(&self) -> NodeState {
        self.node.state()
    }

    pub fn has_iv_runtime(&self) -> bool {
        self.iv_runtime.is_some()
    }

    pub fn has_iv_event_bindings(&self) -> bool {
        self.iv_event_bindings.is_some()
    }

    pub fn iv_source_health(&self, profile_id: &str, source_id: &str) -> Option<IvSourceHealth> {
        self.iv_runtime
            .as_ref()
            .and_then(|runtime| runtime.source_health(profile_id, source_id))
    }

    fn spawn_iv_engine_start_on_running(
        &self,
        root: &BoltV3RootConfig,
    ) -> Result<Option<tokio::task::JoinHandle<()>>, BoltV3LiveNodeError> {
        let Some(iv_runtime) = self.iv_runtime.clone() else {
            return Ok(None);
        };
        if root.iv.is_none() {
            return Ok(None);
        }
        let lifecycle = plan_iv_engine_lifecycle(root).map_err(|error| {
            BoltV3LiveNodeError::StrategyRegistration(
                BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
                    message: format!("bolt-v3 IV lifecycle start planning failed: {error:?}"),
                },
            )
        })?;
        if lifecycle.start_plans.is_empty() {
            return Ok(None);
        }

        let node_handle = self.node.handle();
        let start_plans = lifecycle.start_plans;
        let registered_clients = self.node.kernel().data_engine.borrow().registered_clients();
        let external_clients = root.nautilus.data_engine.external_clients.clone();
        let start_poll_interval =
            Duration::from_millis(root.persistence.runtime_capture_start_poll_interval_ms);
        Ok(Some(tokio::task::spawn_local(async move {
            loop {
                match node_handle.state() {
                    NodeState::Running => break,
                    NodeState::ShuttingDown | NodeState::Stopped => return,
                    NodeState::Idle | NodeState::Starting => {
                        tokio::time::sleep(start_poll_interval).await;
                    }
                }
            }

            let mut adapter =
                NtIvRuntimeCommandSenderAdapter::new(&registered_clients, &external_clients);
            let outcomes = apply_subscription_plans(&mut adapter, &start_plans);
            if let Err(error) = iv_runtime.apply_plan_outcomes(&outcomes) {
                log::error!("bolt-v3 IV lifecycle start outcome update failed: {error:?}");
            }
        })))
    }

    pub fn stop_iv_engine_lifecycle(
        &mut self,
        root: &BoltV3RootConfig,
    ) -> Result<(), BoltV3LiveNodeError> {
        let Some(iv_runtime) = self.iv_runtime.take() else {
            return Ok(());
        };
        let lifecycle = match plan_iv_engine_lifecycle(root) {
            Ok(lifecycle) => lifecycle,
            Err(error) => {
                self.iv_runtime = Some(iv_runtime);
                return Err(BoltV3LiveNodeError::StrategyRegistration(
                    BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
                        message: format!("bolt-v3 IV lifecycle stop planning failed: {error:?}"),
                    },
                ));
            }
        };
        let iv_event_bindings = self.iv_event_bindings.take();
        let outcomes = {
            let mut adapter = NtIvRuntimeBindingAdapter::new(
                &mut self.node,
                &root.nautilus.data_engine.external_clients,
            );
            apply_subscription_plans(&mut adapter, &lifecycle.stop_plans)
        };
        if let Err(error) = iv_runtime.apply_plan_outcomes(&outcomes) {
            self.iv_runtime = Some(iv_runtime);
            self.iv_event_bindings = iv_event_bindings;
            return Err(BoltV3LiveNodeError::StrategyRegistration(
                BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
                    message: format!("bolt-v3 IV lifecycle stop state update failed: {error:?}"),
                },
            ));
        }

        Ok(())
    }

    pub fn registered_data_client_ids(&self) -> Vec<ClientId> {
        self.node.kernel().data_engine.borrow().registered_clients()
    }

    pub fn registration_summary(&self) -> &BoltV3RegistrationSummary {
        &self.registration_summary
    }

    pub fn registered_exec_client_ids(&self) -> Vec<ClientId> {
        self.node.kernel().exec_engine.borrow().client_ids()
    }

    pub fn cached_instrument_ids(&self) -> Vec<String> {
        self.reference_cache_evidence().cached_instrument_ids
    }

    pub fn reference_cache_evidence(&self) -> BoltV3StrategyFreeReferenceCacheEvidence {
        let cache = self.node.kernel().cache();
        let cache = cache.borrow();
        let cached_instrument_ids = cache
            .instrument_ids(None)
            .into_iter()
            .map(ToString::to_string)
            .collect();
        BoltV3StrategyFreeReferenceCacheEvidence {
            cached_instrument_ids,
        }
    }

    pub fn redaction_values(&self) -> &[Zeroizing<String>] {
        &self.redaction_values
    }

    pub fn handle(&self) -> LiveNodeHandle {
        self.node.handle()
    }

    pub fn subscribe_strategy_free_custom_data(
        &mut self,
        client_id: ClientId,
        data_type: DataType,
        params: Params,
    ) -> Result<(), BoltV3LiveNodeError> {
        if !self.registered_data_client_ids().contains(&client_id) {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "custom-data subscription references unregistered data client {client_id}"
                ),
            ));
        }
        let ts_init = self.node.kernel().generate_timestamp_ns();
        let command = SubscribeCustomData::new(
            Some(client_id),
            None,
            data_type,
            UUID4::new(),
            ts_init,
            None,
            Some(params),
        );
        self.node
            .kernel_mut()
            .data_engine
            .borrow_mut()
            .execute(DataCommand::Subscribe(SubscribeCommand::Data(command)));
        Ok(())
    }

    pub fn unsubscribe_strategy_free_custom_data(
        &mut self,
        client_id: ClientId,
        data_type: DataType,
        params: Params,
    ) {
        let ts_init = self.node.kernel().generate_timestamp_ns();
        let command = UnsubscribeCustomData::new(
            Some(client_id),
            None,
            data_type,
            UUID4::new(),
            ts_init,
            None,
            Some(params),
        );
        self.node
            .kernel_mut()
            .data_engine
            .borrow_mut()
            .execute(DataCommand::Unsubscribe(UnsubscribeCommand::Data(command)));
    }

    pub fn subscribe_strategy_free_quotes(
        &mut self,
        client_id: ClientId,
        instrument_id: InstrumentId,
    ) -> Result<(), BoltV3LiveNodeError> {
        self.ensure_strategy_free_data_client_registered(client_id, "quote")?;
        let ts_init = self.node.kernel().generate_timestamp_ns();
        let command = SubscribeQuotes::new(
            instrument_id,
            Some(client_id),
            None,
            UUID4::new(),
            ts_init,
            None,
            None,
        );
        self.node
            .kernel_mut()
            .data_engine
            .borrow_mut()
            .execute(DataCommand::Subscribe(SubscribeCommand::Quotes(command)));
        Ok(())
    }

    pub fn unsubscribe_strategy_free_quotes(
        &mut self,
        client_id: ClientId,
        instrument_id: InstrumentId,
    ) {
        let ts_init = self.node.kernel().generate_timestamp_ns();
        let command = UnsubscribeQuotes::new(
            instrument_id,
            Some(client_id),
            None,
            UUID4::new(),
            ts_init,
            None,
            None,
        );
        self.node
            .kernel_mut()
            .data_engine
            .borrow_mut()
            .execute(DataCommand::Unsubscribe(UnsubscribeCommand::Quotes(
                command,
            )));
    }

    pub fn subscribe_strategy_free_book_deltas(
        &mut self,
        client_id: ClientId,
        instrument_id: InstrumentId,
        book_type: BookType,
    ) -> Result<(), BoltV3LiveNodeError> {
        self.ensure_strategy_free_data_client_registered(client_id, "book")?;
        let ts_init = self.node.kernel().generate_timestamp_ns();
        let command = SubscribeBookDeltas::new(
            instrument_id,
            book_type,
            Some(client_id),
            None,
            UUID4::new(),
            ts_init,
            None,
            false,
            None,
            None,
        );
        self.node
            .kernel_mut()
            .data_engine
            .borrow_mut()
            .execute(DataCommand::Subscribe(SubscribeCommand::BookDeltas(
                command,
            )));
        Ok(())
    }

    pub fn unsubscribe_strategy_free_book_deltas(
        &mut self,
        client_id: ClientId,
        instrument_id: InstrumentId,
    ) {
        let ts_init = self.node.kernel().generate_timestamp_ns();
        let command = UnsubscribeBookDeltas::new(
            instrument_id,
            Some(client_id),
            None,
            UUID4::new(),
            ts_init,
            None,
            None,
        );
        self.node
            .kernel_mut()
            .data_engine
            .borrow_mut()
            .execute(DataCommand::Unsubscribe(UnsubscribeCommand::BookDeltas(
                command,
            )));
    }

    pub fn subscribe_strategy_free_trades(
        &mut self,
        client_id: ClientId,
        instrument_id: InstrumentId,
    ) -> Result<(), BoltV3LiveNodeError> {
        self.ensure_strategy_free_data_client_registered(client_id, "trade")?;
        let ts_init = self.node.kernel().generate_timestamp_ns();
        let command = SubscribeTrades::new(
            instrument_id,
            Some(client_id),
            None,
            UUID4::new(),
            ts_init,
            None,
            None,
        );
        self.node
            .kernel_mut()
            .data_engine
            .borrow_mut()
            .execute(DataCommand::Subscribe(SubscribeCommand::Trades(command)));
        Ok(())
    }

    pub fn unsubscribe_strategy_free_trades(
        &mut self,
        client_id: ClientId,
        instrument_id: InstrumentId,
    ) {
        let ts_init = self.node.kernel().generate_timestamp_ns();
        let command = UnsubscribeTrades::new(
            instrument_id,
            Some(client_id),
            None,
            UUID4::new(),
            ts_init,
            None,
            None,
        );
        self.node
            .kernel_mut()
            .data_engine
            .borrow_mut()
            .execute(DataCommand::Unsubscribe(UnsubscribeCommand::Trades(
                command,
            )));
    }

    fn ensure_strategy_free_data_client_registered(
        &self,
        client_id: ClientId,
        market_data_kind: &str,
    ) -> Result<(), BoltV3LiveNodeError> {
        if self.registered_data_client_ids().contains(&client_id) {
            return Ok(());
        }
        Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
            anyhow::anyhow!(
                "{market_data_kind} subscription references unregistered data client {client_id}"
            ),
        ))
    }

    pub async fn run_strategy_free_until_stop_or_timeout(
        &mut self,
        run_timeout: Duration,
        stop_timeout: Duration,
    ) -> Result<bool, BoltV3LiveNodeError> {
        let handle = self.node.handle();
        let run_future = self.node.run();
        tokio::pin!(run_future);

        match tokio::time::timeout(run_timeout, &mut run_future).await {
            Ok(result) => {
                result.map_err(BoltV3LiveNodeError::StrategyFreeStartFailed)?;
                Ok(false)
            }
            Err(_) => {
                handle.stop();
                tokio::time::timeout(stop_timeout, run_future)
                    .await
                    .map_err(|_| BoltV3LiveNodeError::StrategyFreeStopTimeout {
                        timeout_secs: stop_timeout.as_secs(),
                    })?
                    .map_err(BoltV3LiveNodeError::StrategyFreeStopFailed)?;
                Ok(true)
            }
        }
    }

    pub async fn run_strategy_free_until_running_then_stop(
        &mut self,
        start_timeout: Duration,
        stop_timeout: Duration,
        poll_interval: Duration,
    ) -> Result<(), BoltV3LiveNodeError> {
        let handle = self.node.handle();
        let run_future = self.node.run();
        tokio::pin!(run_future);
        let deadline = tokio::time::sleep(start_timeout);
        tokio::pin!(deadline);

        loop {
            match handle.state() {
                NodeState::Running => break,
                NodeState::ShuttingDown | NodeState::Stopped => {
                    return Err(BoltV3LiveNodeError::StrategyFreeStartIncomplete);
                }
                NodeState::Idle | NodeState::Starting => {}
            }

            let sleep = tokio::time::sleep(poll_interval);
            tokio::pin!(sleep);
            tokio::select! {
                result = &mut run_future => {
                    result.map_err(BoltV3LiveNodeError::StrategyFreeStartFailed)?;
                    return Err(BoltV3LiveNodeError::StrategyFreeStartIncomplete);
                }
                _ = &mut deadline => {
                    handle.stop();
                    tokio::time::timeout(stop_timeout, run_future)
                        .await
                        .map_err(|_| BoltV3LiveNodeError::StrategyFreeStopTimeout {
                            timeout_secs: stop_timeout.as_secs(),
                        })?
                        .map_err(BoltV3LiveNodeError::StrategyFreeStopFailed)?;
                    return Err(BoltV3LiveNodeError::StrategyFreeStartTimeout {
                        timeout_secs: start_timeout.as_secs(),
                    });
                }
                _ = &mut sleep => {}
            }
        }

        handle.stop();
        tokio::time::timeout(stop_timeout, run_future)
            .await
            .map_err(|_| BoltV3LiveNodeError::StrategyFreeStopTimeout {
                timeout_secs: stop_timeout.as_secs(),
            })?
            .map_err(BoltV3LiveNodeError::StrategyFreeStopFailed)?;
        Ok(())
    }

    pub fn instance_id(&self) -> String {
        self.node.instance_id().to_string()
    }

    pub fn admitted_order_count(&self) -> u32 {
        self.submit_admission.admitted_order_count()
    }

    pub fn loss_governor_configured(&self) -> bool {
        self.submit_admission.loss_governor_configured()
    }

    pub fn loss_governor_runtime_feed_configured(&self) -> bool {
        self.loss_runtime_feed.is_some() && self.loss_runtime_feed_subscription.is_some()
    }

    pub fn kill_switch_loss_protection_configured(&self) -> bool {
        self.loss_protection.is_some()
    }

    pub fn order_reject_observer_feed_configured(&self) -> bool {
        self.order_reject_observer_feed.is_some()
            && self.order_reject_observer_feed_subscription.is_some()
    }

    pub fn provider_collateral_allowance_runtime_configured(&self) -> bool {
        self.provider_collateral_allowance_runtime_guard.is_some()
    }

    pub fn operator_health_surface(
        &self,
        input_health: Option<BoltV3InputHealth>,
    ) -> Result<BoltV3OperatorHealthSurface> {
        let settlement_health = settlement_health_snapshot(&self.settlement_health)?;
        Ok(live_operator_health_surface(
            self.order_reject_observer_feed.as_ref(),
            &self.submit_admission,
            self.capital_admission_runtime_feed.is_some(),
            self.input_health_configured_source_count,
            input_health,
            settlement_health,
            &self.decision_evidence,
        ))
    }

    pub fn emit_operator_health_surface_transition(&self, reason: &'static str) {
        match self.operator_health_surface(None) {
            Ok(surface) => {
                self.operator_health_transition_logger
                    .emit_surface(reason, surface);
            }
            Err(error) => {
                log::error!(
                    "operator health surface snapshot failed: reason={reason} error={error:#}"
                );
            }
        }
    }

    fn decision_evidence_producer_guards_for_shutdown(
        &mut self,
        loss_protection_guards: BoltV3LossProtectionRuntimeGuards,
    ) -> BoltV3DecisionEvidenceProducerGuards {
        BoltV3DecisionEvidenceProducerGuards {
            loss_protection_guards,
            loss_runtime_feed_subscription: self.loss_runtime_feed_subscription.take(),
            order_reject_observer_feed_subscription: self
                .order_reject_observer_feed_subscription
                .take(),
            submit_admission_nt_projection_subscription: self
                .submit_admission_nt_projection_subscription
                .take(),
            provider_collateral_allowance_runtime_guard: self
                .provider_collateral_allowance_runtime_guard
                .take(),
        }
    }

    async fn drain_decision_evidence_shutdown<P>(
        &self,
        producer_guards: P,
    ) -> Result<(), BoltV3LiveNodeError>
    where
        P: BoltV3DecisionEvidenceProducerStopper,
    {
        drain_after_stopping_decision_evidence_producers(producer_guards, || {
            self.decision_evidence_runtime.close();
            Ok(())
        })
        .await
        .map_err(BoltV3LiveNodeError::DecisionEvidenceShutdownDrain)
    }

    pub fn nt_risk_trading_state(&self) -> TradingState {
        self.node.kernel().risk_engine().borrow().trading_state()
    }

    /// NT `RiskEngine` trading state. Alias of [`nt_risk_trading_state`] kept
    /// for the kill-switch durable-recovery tests, which assert the seeded
    /// kill-switch state is synced into NT's trading state at build time.
    pub fn nt_trading_state(&self) -> TradingState {
        self.node.kernel().risk_engine().borrow().trading_state()
    }

    /// Kind of the currently latched durable kill-switch state.
    ///
    /// Derived from the configured loss-protection accumulator, which owns the
    /// durable kill-switch state machine and was seeded from the durable store
    /// at build time. When the hard kill switch is disabled there is no
    /// accumulator, so the runtime reports the armed (non-latched) default.
    pub fn kill_switch_state_kind(&self) -> KillSwitchStateKind {
        match self.loss_protection.as_ref() {
            Some(loss_protection) => loss_protection.borrow().state().kind(),
            None => KillSwitchStateKind::Armed,
        }
    }

    pub fn apply_loss_governor_manual_recovery(
        &self,
        evidence: &LossGovernorManualRecoveryEvidence,
        now_ns: u64,
    ) -> Option<TradingState> {
        let loss_policy = self.submit_admission.loss_governor_policy()?;
        let action_policy = self.loss_halt_action_policy.as_ref()?;
        let snapshot = self.submit_admission.loss_snapshot();
        let decision = evaluate_loss_admission(&loss_policy, snapshot.as_ref(), now_ns);
        let current_state = self.nt_risk_trading_state();
        let target =
            next_loss_governor_manual_recovery_trading_state(LossGovernorManualRecoveryRequest {
                policy: action_policy,
                current_state,
                decision: &decision,
                snapshot: snapshot.as_ref(),
                now_ns,
                max_snapshot_age_ns: loss_policy.max_snapshot_age_ns,
                evidence: Some(evidence),
                max_evidence_path_bytes: action_policy.manual_recovery_evidence_max_path_bytes,
            })?;
        self.node
            .kernel()
            .risk_engine()
            .borrow_mut()
            .set_trading_state(target);
        Some(target)
    }

    pub fn capital_admission_configured(&self) -> bool {
        self.submit_admission.capital_admission_configured()
    }

    pub fn capital_admission_runtime_feed_configured(&self) -> bool {
        self.capital_admission_runtime_feed.is_some()
            && self.submit_admission_nt_projection_subscription.is_some()
    }

    pub fn capital_admission_reconciled(&self) -> Option<bool> {
        self.submit_admission.capital_admission_reconciled()
    }

    /// Rebuild the capital admission's capital-reservation ledger from the
    /// live NT cache at startup so a restart cannot double-allocate capital
    /// against orders/positions that already exist. Reads open orders,
    /// the configured collateral balance, and open positions for the
    /// configured account, seeds the runtime feed's portfolio/cache
    /// snapshot from that same observation, then attributes each open order
    /// to recovered reservation metadata (when configured) before handing a
    /// single coherent snapshot to submit admission. If any open order
    /// cannot be attributed, the snapshot is marked not-all-attributed so
    /// the caller fails closed rather than arming with an unreconciled
    /// ledger.
    #[cfg(test)]
    pub fn rebuild_capital_admission_from_nt_cache(
        &self,
        now_ns: u64,
    ) -> BoltV3SubmitCapitalAdmissionRebuildDecision {
        Self::rebuild_capital_admission_from_nt_cache_parts(
            &self.node.kernel().cache(),
            self.capital_admission_runtime_feed.as_ref(),
            self.submit_reservation_recovery.as_ref(),
            &self.submit_admission_nt_reconciliation_account_ids,
            self.submit_admission.as_ref(),
            now_ns,
        )
    }

    fn rebuild_capital_admission_from_nt_cache_parts(
        cache: &Rc<RefCell<Cache>>,
        capital_admission_runtime_feed: Option<&Arc<Mutex<CapitalAdmissionRuntimeFeed>>>,
        submit_reservation_recovery: &ReservationRecoveryFacts,
        reconciliation_account_ids: &BTreeSet<AccountId>,
        submit_admission: &BoltV3SubmitAdmissionState,
        now_ns: u64,
    ) -> BoltV3SubmitCapitalAdmissionRebuildDecision {
        let (
            account_id,
            binary_instrument_ids,
            collateral_currency,
            accepted_allowance_observed_at_ns,
        ) = match capital_admission_runtime_feed {
            Some(feed) => {
                let feed = feed
                    .lock()
                    .expect("capital admission rebuild configuration feed lock poisoned");
                (
                    Some(feed.configured_account_id()),
                    feed.configured_binary_instrument_ids(),
                    Some(feed.configured_collateral_currency()),
                    feed.accepted_allowance_observed_at_ns(),
                )
            }
            None => (None, None, None, None),
        };
        let cache = cache.borrow();
        let open_order_snapshots = reconciliation_account_ids
            .iter()
            .flat_map(|account_id| {
                cache
                    .orders_open(None, None, None, Some(account_id), None)
                    .into_iter()
                    .map(|order| order.cloned())
            })
            .collect::<Vec<_>>();
        let open_client_order_ids = open_order_snapshots
            .iter()
            .map(|order| order.client_order_id().to_string())
            .collect::<Vec<_>>();
        let unique_open_client_order_ids =
            open_client_order_ids.iter().collect::<BTreeSet<_>>().len()
                == open_client_order_ids.len();
        let cached_account_balances = match (account_id.as_ref(), collateral_currency.as_deref()) {
            (Some(account_id), Some(collateral_currency)) => {
                cache.account_owned(account_id).and_then(|account| {
                    let balances = account.balances();
                    balances
                        .values()
                        .find(|balance| balance.currency.code.as_str() == collateral_currency)
                        .map(|balance| (balance.free.as_decimal(), balance.total.as_decimal()))
                })
            }
            _ => None,
        };
        let missing_nt_account_cache_balance = match (
            account_id.as_ref(),
            collateral_currency.as_deref(),
        ) {
            (Some(account_id), Some(collateral_currency)) if cached_account_balances.is_none() => {
                let missing = BoltV3SubmitCapitalAdmissionMissingNtAccountCacheBalance {
                    account_id: account_id.to_string(),
                    collateral_currency: collateral_currency.to_string(),
                };
                log::warn!(
                    "bolt-v3 capital admission startup rebuild could not seed account portfolio snapshot because NT cache is missing account_id={} collateral_currency={}",
                    missing.account_id,
                    missing.collateral_currency
                );
                Some(missing)
            }
            _ => None,
        };
        let (yes_position, no_position) =
            match (account_id.as_ref(), binary_instrument_ids.as_ref()) {
                (Some(account_id), Some((yes_instrument_id, no_instrument_id))) => {
                    let mut yes_position = Decimal::ZERO;
                    let mut no_position = Decimal::ZERO;
                    for position in cache.positions_open(None, None, None, Some(account_id), None) {
                        let instrument_id = position.instrument_id.to_string();
                        if instrument_id == *yes_instrument_id {
                            yes_position += position.signed_decimal_qty();
                        } else if instrument_id == *no_instrument_id {
                            no_position += position.signed_decimal_qty();
                        }
                    }
                    (yes_position, no_position)
                }
                _ => (Decimal::ZERO, Decimal::ZERO),
            };
        drop(cache);

        let canonical_projection = CapitalAdmissionNtCacheProjection {
            accepted_allowance_observed_at_ns,
            account_balances: cached_account_balances,
            open_client_order_ids: open_client_order_ids.clone(),
            yes_position,
            no_position,
            observed_at_ns: now_ns,
        };
        let canonical_components = capital_admission_runtime_feed.map(|feed| {
            feed.lock()
                .expect("capital admission canonical NT projection feed lock poisoned")
                .canonical_nt_components(canonical_projection.clone())
        });
        let projection_complete = canonical_components
            .as_ref()
            .is_none_or(std::result::Result::is_ok)
            && unique_open_client_order_ids;
        if let Some(Ok(components)) = canonical_components.as_ref() {
            submit_admission.update_capital_admission_nt_components(components.clone());
        } else if let Some(Err(error)) = canonical_components.as_ref() {
            log::warn!("capital admission canonical NT projection rejected: {error:?}");
        }

        let mut reservations = Vec::with_capacity(open_order_snapshots.len());
        let mut live_non_reservation_client_order_ids = BTreeSet::new();
        let mut live_forced_reduction_client_order_ids = BTreeSet::new();
        let mut all_open_orders_attributed = projection_complete;
        for order in &open_order_snapshots {
            let Some(evidence) = nt_open_order_evidence_from_order(order, now_ns) else {
                all_open_orders_attributed = false;
                break;
            };
            let client_order_id = evidence.client_order_id.clone();
            if submit_reservation_recovery.authorizes_non_reservation_order(&client_order_id) {
                if submit_reservation_recovery.authorizes_forced_reduction_order(&client_order_id) {
                    live_forced_reduction_client_order_ids.insert(client_order_id.clone());
                }
                live_non_reservation_client_order_ids.insert(client_order_id);
                continue;
            }
            let Some(metadata) =
                submit_reservation_recovery.reservation_attribution(&client_order_id)
            else {
                all_open_orders_attributed = false;
                break;
            };
            let fill_trade_ids = submit_reservation_recovery
                .reservation_fill_trade_ids(&client_order_id, &metadata.submit_reservation_id)
                .cloned()
                .unwrap_or_default();
            let Some(reservation) = submit_admission
                .capital_admission_open_order_reservation_from_attribution(
                    evidence,
                    metadata,
                    &fill_trade_ids,
                )
            else {
                all_open_orders_attributed = false;
                break;
            };
            reservations.push(reservation);
        }
        if !all_open_orders_attributed {
            reservations.clear();
            live_non_reservation_client_order_ids.clear();
            live_forced_reduction_client_order_ids.clear();
        }

        let mut rebuild = submit_admission.rebuild_capital_admission_open_order_snapshot(
            BoltV3SubmitCapitalAdmissionOpenOrderSnapshot {
                observed_at_ns: now_ns,
                evidence_source: CapitalAdmissionRebuildSource::NtOpenOrderCache,
                observed_open_order_count: open_order_snapshots.len(),
                all_open_orders_attributed,
                reservations,
                live_non_reservation_client_order_ids,
                live_forced_reduction_client_order_ids,
            },
            now_ns,
        );
        if let Some(missing) = missing_nt_account_cache_balance {
            rebuild = rebuild.with_missing_nt_account_cache_balance(
                missing.account_id,
                missing.collateral_currency,
            );
        }
        if let Some(Ok(mut components)) = canonical_components {
            components.order_lifecycle.all_open_orders_attributed =
                all_open_orders_attributed && rebuild.accepted;
            if rebuild.accepted
                && let Some(accepted_allowance_observed_at_ns) = accepted_allowance_observed_at_ns
            {
                submit_admission
                    .update_capital_admission_nt_components_after_accepted_allowance_snapshot(
                        components,
                        accepted_allowance_observed_at_ns,
                    );
            } else {
                submit_admission.update_capital_admission_nt_components(components);
            }
        }
        rebuild
    }
}

fn nt_open_order_evidence_from_order(
    order: &OrderAny,
    observed_at_ns: u64,
) -> Option<BoltV3SubmitCapitalAdmissionOpenOrderEvidence> {
    if order.order_type() != OrderType::Limit {
        return None;
    }
    let side = match order.order_side() {
        OrderSide::Buy => BoltV3CompiledOrderSide::Buy,
        OrderSide::Sell => BoltV3CompiledOrderSide::Sell,
        _ => return None,
    };
    let limit_price = order.price()?.as_decimal();
    if !(Decimal::ZERO..=Decimal::ONE).contains(&limit_price) {
        return None;
    }
    let open_quantity = order.leaves_qty().as_decimal();
    if open_quantity <= Decimal::ZERO {
        return None;
    }
    Some(BoltV3SubmitCapitalAdmissionOpenOrderEvidence {
        client_order_id: order.client_order_id().to_string(),
        instrument_id: order.instrument_id().to_string(),
        side,
        open_quantity,
        limit_price,
        observed_at_ns,
        evidence_label: "nt_open_order_cache".to_string(),
    })
}

impl std::fmt::Debug for BoltV3LiveNodeRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoltV3LiveNodeRuntime")
            .field("node", &"[redacted]")
            .field("submit_admission", &self.submit_admission)
            .field("redaction_values", &"[redacted]")
            .finish()
    }
}

#[derive(Debug)]
pub enum BoltV3LiveNodeBuilderError {
    BuilderConstruction { source: anyhow::Error },
}

impl std::fmt::Display for BoltV3LiveNodeBuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3LiveNodeBuilderError::BuilderConstruction { source } => {
                write!(f, "NT LiveNodeBuilder construction failed: {source}")
            }
        }
    }
}

impl std::error::Error for BoltV3LiveNodeBuilderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3LiveNodeBuilderError::BuilderConstruction { source } => Some(source.as_ref()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveNodeStartupShutdownGraceTrigger {
    StartupDeadline,
    RuntimeCaptureFailure,
    /// Stop was requested because `NodeState::Running` without a started trader
    /// was observed (engines-not-connected fail-open). The startup shutdown
    /// grace elapsed before the runner returned; the launch cause is still the
    /// trader-not-started invariant, not a generic startup/connect timeout.
    TraderNotStartedInvariant,
}

impl std::fmt::Display for LiveNodeStartupShutdownGraceTrigger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiveNodeStartupShutdownGraceTrigger::StartupDeadline => f.write_str("startup deadline"),
            LiveNodeStartupShutdownGraceTrigger::RuntimeCaptureFailure => {
                f.write_str("runtime capture failure")
            }
            LiveNodeStartupShutdownGraceTrigger::TraderNotStartedInvariant => {
                f.write_str("trader-not-started launch invariant")
            }
        }
    }
}

#[derive(Debug)]
pub enum BoltV3LiveNodeError {
    ForbiddenEnv(ForbiddenEnvVarError),
    /// `SsmResolverSession::new()` failed before any client secret was
    /// read. The wrapped `SecretError` is the upstream Tokio /
    /// AWS-SDK-config setup failure. Distinct from
    /// [`SecretResolution`] (which carries a per-client `BoltV3SecretError`
    /// with client key, secret-config field name, and SSM path) because
    /// session setup happens before any client path is consulted, so an
    /// operator message that names a client or SSM path would be wrong.
    SecretResolverSetup(crate::secrets::SecretError),
    SecretResolution(BoltV3SecretError),
    AdapterMapping(BoltV3AdapterMappingError),
    BuilderConstruction(BoltV3LiveNodeBuilderError),
    ClientRegistration(BoltV3ClientRegistrationError),
    StrategyRegistration(BoltV3StrategyRegistrationError),
    RiskPolicy(anyhow::Error),
    Build(anyhow::Error),
    /// Enabled kill-switch startup recovery found durable state that cannot
    /// safely re-arm local admission. The build path fails closed before
    /// resolving secrets, constructing NT clients, or registering
    /// submit-capable strategy runtime.
    KillSwitchRecovery {
        reason: KillSwitchRecoveryReason,
    },
    /// Enabled kill-switch startup recovery could not read the durable state
    /// store. Distinct from a classified fail-closed state because the
    /// underlying I/O error is useful operator evidence.
    KillSwitchStore(KillSwitchStoreError),
    /// Enabled kill-switch loss protection (the durable daily-realized
    /// accumulator) could not be configured, seeded from the durable store, or
    /// persisted at build time. Fails the build closed rather than running
    /// without the hard daily-realized circuit breaker.
    KillSwitchLossProtection(anyhow::Error),
    /// Provider-specific live-submit approval loading or consumption failed
    /// while building the adapter bundle. This is intentionally outside the
    /// live runner wrapper; production `run_bolt_v3_live_node` still enters NT
    /// without reintroducing the removed start gate.
    OperatorApprovalConsumption(anyhow::Error),
    /// The loaded root TOML configured clients beyond the selected
    /// strategy-owned transport path, but the strategy-owned
    /// execution/reference client set could not be derived or validated
    /// against `[clients]`.
    LiveTransportScope {
        reason: String,
    },
    /// NT returned an error from `LiveNode::run`.
    Run(anyhow::Error),
    /// NT runtime capture could not be wired from the validated
    /// bolt-v3 `[persistence]` config before the runner loop started.
    RuntimeCaptureWire(anyhow::Error),
    /// NT runtime capture failed during shutdown after the runner loop
    /// exited or after the capture worker asked the LiveNode to stop.
    RuntimeCaptureShutdown(anyhow::Error),
    /// NT's runner loop and runtime-capture shutdown both failed. This
    /// preserves both failure categories instead of reporting the
    /// compound case as only a capture-shutdown error.
    RunAndRuntimeCaptureShutdown {
        run_error: anyhow::Error,
        shutdown_error: anyhow::Error,
    },
    /// Decision-evidence shutdown drain failed after the NT runner and runtime
    /// capture shutdown path completed. This is a fail-loud data-loss boundary:
    /// buffered or kernel-resident evidence records must be flushed before the
    /// live runner reports success.
    DecisionEvidenceShutdownDrain(anyhow::Error),
    /// The runner/capture path failed and the decision-evidence shutdown drain
    /// also failed. Preserve both categories so the evidence loss cannot be
    /// hidden behind the earlier runner error.
    RunAndDecisionEvidenceShutdownDrain {
        run_error: Box<BoltV3LiveNodeError>,
        drain_error: anyhow::Error,
    },
    /// The bolt-v3 controlled-connect boundary
    /// ([`connect_bolt_v3_clients`]) bounds NT client connection by the
    /// config-owned Nautilus connection timeout. A `ConnectTimeout` is
    /// surfaced when that bound elapses before NT reports every registered
    /// client connected, instead of client startup hanging indefinitely.
    /// This boundary can inspect the kernel after timeout, so the client list
    /// is the exact not-connected client set at that boundary.
    /// The wrapped value is the configured timeout the boundary
    /// applied (in seconds), captured so log/audit consumers can
    /// distinguish a 1-second test timeout from a 30-second
    /// production timeout without re-reading the source config.
    ConnectTimeout {
        timeout_secs: u64,
        node_state: String,
        not_connected_client_labels: Vec<String>,
    },
    /// Production live-node startup did not reach NT `Running` before the
    /// config-derived Nautilus startup bound. The runner owns the mutable NT
    /// node during `LiveNode::run()`, so this variant names the registered
    /// startup clients rather than claiming an exact disconnected set.
    LiveNodeStartupTimeout {
        timeout_secs: u64,
        node_state: String,
        registered_client_labels: Vec<String>,
    },
    /// Live-node startup requested shutdown before NT's runner completed, and
    /// the bounded startup shutdown grace elapsed before the runner returned.
    /// This names the elapsed shutdown-grace bound instead of reporting a
    /// fresh startup/connect timeout.
    LiveNodeStartupShutdownGraceTimeout {
        trigger: LiveNodeStartupShutdownGraceTrigger,
        shutdown_grace: Duration,
        node_state: String,
        registered_client_labels: Vec<String>,
    },
    /// NT reached `NodeState::Running` without starting the trader.
    ///
    /// This is the engines-not-connected fail-open inside NT's live node: it
    /// logs "Not starting trader: engine client(s) not connected" (or the
    /// connect-timeout equivalent), sets `NodeState::Running`, and idles a
    /// trader-less process. Bolt treats that as a launch failure — same
    /// invariant family as boot-aborts-if-mute. No retries, no thresholds.
    LiveNodeTraderNotStarted {
        node_state: String,
        registered_client_labels: Vec<String>,
    },
    /// The bolt-v3 controlled-connect boundary dispatched both NT
    /// engine-level connect futures within the configured bound, but
    /// at least one registered NT data or execution client did not
    /// transition to `is_connected` afterwards. The pinned NT
    /// `DataEngine::connect` and `ExecutionEngine::connect`
    /// dispatchers swallow individual client `connect()` errors and
    /// only log them, so bolt-v3 consults
    /// `NautilusKernel::check_engines_connected()` after dispatch
    /// returns to keep this failure mode honest. This slice keeps the
    /// variant generic rather than synthesizing a per-client failure
    /// list. Callers should follow this with a
    /// [`disconnect_bolt_v3_clients`] call to drain any partially
    /// connected clients under the bounded controlled-disconnect
    /// boundary.
    ConnectIncomplete,
    /// The bolt-v3 controlled-disconnect boundary
    /// ([`disconnect_bolt_v3_clients`]) bounds the
    /// `NautilusKernel::disconnect_clients` future by the
    /// `nautilus.timeout_disconnection_secs` value from the loaded
    /// bolt-v3 config. A `DisconnectTimeout` is surfaced when that
    /// bound elapses before NT finishes disconnecting all data and
    /// execution clients, instead of the controlled-disconnect call
    /// hanging indefinitely. The wrapped value is the configured
    /// timeout the boundary applied (in seconds).
    DisconnectTimeout {
        timeout_secs: u64,
    },
    /// The bolt-v3 controlled-disconnect boundary dispatched
    /// `NautilusKernel::disconnect_clients` and NT returned an
    /// `Err(..)` from at least one registered client's `disconnect()`
    /// call. The wrapped `anyhow::Error` is the value NT bubbled up
    /// from its engine-level disconnect aggregator.
    DisconnectFailed(anyhow::Error),
    StrategyFreeStartTimeout {
        timeout_secs: u64,
    },
    StrategyFreeStartTimeoutOverflow,
    StrategyFreeStartIncomplete,
    StrategyFreeExecutionAccountsMissing {
        client_venues: Vec<String>,
    },
    StrategyFreeReferenceProbeSetup(anyhow::Error),
    StrategyFreeReferenceProbeFailed {
        reason: String,
    },
    StrategyFreeDataClientProbeFailed {
        reason: String,
    },
    StrategyFreeStartFailed(anyhow::Error),
    StrategyFreeStopTimeout {
        timeout_secs: u64,
    },
    StrategyFreeStopTimeoutOverflow,
    StrategyFreeStopFailed(anyhow::Error),
    /// The startup capital-admission rebuild from the NT cache could not
    /// attribute one or more pre-existing open orders to recovered
    /// atomic admitted reservation attribution, so submit admission would arm with an
    /// unreconciled capital-reservation ledger. The live runner refuses to
    /// enter NT's loop in this state to avoid double-allocating capital
    /// against orders it cannot account for. The wrapped decision carries
    /// the attempted/rebuilt reservation counts and rejection reason
    /// captured at boot. This is intentionally outside the removed start
    /// gate: it is a fail-closed reconciliation guard, not the live-canary
    /// arm gate, so it never reintroduces a gate-report/arm sequence.
    StartupCapitalAdmissionRebuild(BoltV3SubmitCapitalAdmissionRebuildDecision),
}

impl std::fmt::Display for BoltV3LiveNodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BoltV3LiveNodeError::ForbiddenEnv(error) => write!(f, "{error}"),
            BoltV3LiveNodeError::SecretResolverSetup(error) => write!(
                f,
                "bolt-v3 SSM resolver session setup failed before any client \
                 secret could be read: {error}"
            ),
            BoltV3LiveNodeError::SecretResolution(error) => {
                write!(f, "bolt-v3 secret resolution failed: {error}")
            }
            BoltV3LiveNodeError::AdapterMapping(error) => {
                write!(f, "bolt-v3 adapter config mapping failed: {error}")
            }
            BoltV3LiveNodeError::BuilderConstruction(error) => write!(f, "{error}"),
            BoltV3LiveNodeError::ClientRegistration(error) => {
                write!(f, "bolt-v3 client registration failed: {error}")
            }
            BoltV3LiveNodeError::StrategyRegistration(error) => {
                write!(f, "bolt-v3 strategy registration failed: {error}")
            }
            BoltV3LiveNodeError::RiskPolicy(error) => {
                write!(f, "bolt-v3 risk policy mapping failed: {error}")
            }
            BoltV3LiveNodeError::Build(error) => write!(f, "LiveNode build failed: {error}"),
            BoltV3LiveNodeError::KillSwitchRecovery { reason } => write!(
                f,
                "bolt-v3 kill-switch durable state recovery failed closed before live-node build: {reason}"
            ),
            BoltV3LiveNodeError::KillSwitchStore(error) => write!(
                f,
                "bolt-v3 kill-switch durable state store read failed before live-node build: {error}"
            ),
            BoltV3LiveNodeError::KillSwitchLossProtection(error) => write!(
                f,
                "bolt-v3 kill-switch loss protection setup failed: {error}"
            ),
            BoltV3LiveNodeError::OperatorApprovalConsumption(error) => {
                write!(
                    f,
                    "bolt-v3 provider live-submit approval consumption failed: {error}"
                )
            }
            BoltV3LiveNodeError::LiveTransportScope { reason } => write!(
                f,
                "bolt-v3 live transport scope could not be derived from strategy-owned client bindings: {reason}"
            ),
            BoltV3LiveNodeError::Run(error) => write!(f, "LiveNode run failed: {error}"),
            BoltV3LiveNodeError::RuntimeCaptureWire(error) => {
                write!(f, "NT runtime capture wiring failed: {error}")
            }
            BoltV3LiveNodeError::RuntimeCaptureShutdown(error) => {
                write!(f, "NT runtime capture shutdown failed: {error}")
            }
            BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown {
                run_error,
                shutdown_error,
            } => write!(
                f,
                "LiveNode run failed and NT runtime capture shutdown failed: \
                 run error: {run_error}; shutdown error: {shutdown_error}"
            ),
            BoltV3LiveNodeError::DecisionEvidenceShutdownDrain(error) => {
                write!(
                    f,
                    "bolt-v3 decision evidence shutdown drain failed: {error}"
                )
            }
            BoltV3LiveNodeError::RunAndDecisionEvidenceShutdownDrain {
                run_error,
                drain_error,
            } => write!(
                f,
                "LiveNode run, runtime-capture, or IV lifecycle stop failed and bolt-v3 decision \
                 evidence shutdown drain failed: run error: {run_error}; drain error: {drain_error}"
            ),
            BoltV3LiveNodeError::ConnectTimeout {
                timeout_secs,
                node_state,
                not_connected_client_labels,
            } => write!(
                f,
                "bolt-v3 controlled-connect exceeded the configured Nautilus connection \
                 timeout bound ({timeout_secs}s); node_state={node_state}; \
                 not_connected_client_labels={}",
                live_node_client_list_for_display(not_connected_client_labels)
            ),
            BoltV3LiveNodeError::LiveNodeStartupTimeout {
                timeout_secs,
                node_state,
                registered_client_labels,
            } => write!(
                f,
                "bolt-v3 live-node startup exceeded the configured Nautilus startup \
                 timeout bound ({timeout_secs}s); node_state={node_state}; \
                 registered_client_labels={}",
                live_node_client_list_for_display(registered_client_labels)
            ),
            BoltV3LiveNodeError::LiveNodeStartupShutdownGraceTimeout {
                trigger,
                shutdown_grace,
                node_state,
                registered_client_labels,
            } => {
                // Keep the trader-not-started cause in the operator-visible string even
                // when the runner hung through the shutdown grace — do not look like a
                // generic startup timeout.
                if matches!(
                    trigger,
                    LiveNodeStartupShutdownGraceTrigger::TraderNotStartedInvariant
                ) {
                    write!(
                        f,
                        "bolt-v3 live-node launch aborted: NT NodeState is Running but the \
                         trader was never started (engines-not-connected fail-open); \
                         startup shutdown grace elapsed after {trigger} ({shutdown_grace:?}) \
                         before the runner returned; node_state={node_state}; \
                         registered_client_labels={}",
                        live_node_client_list_for_display(registered_client_labels)
                    )
                } else {
                    write!(
                        f,
                        "bolt-v3 live-node startup shutdown grace elapsed after {trigger} \
                         ({shutdown_grace:?}); node_state={node_state}; \
                         registered_client_labels={}",
                        live_node_client_list_for_display(registered_client_labels)
                    )
                }
            }
            BoltV3LiveNodeError::LiveNodeTraderNotStarted {
                node_state,
                registered_client_labels,
            } => write!(
                f,
                "bolt-v3 live-node launch aborted: NT NodeState is Running but the trader \
                 was never started (engines-not-connected fail-open); \
                 node_state={node_state}; registered_client_labels={}",
                live_node_client_list_for_display(registered_client_labels)
            ),
            BoltV3LiveNodeError::ConnectIncomplete => write!(
                f,
                "bolt-v3 controlled-connect dispatched both NT engine-level connect \
                 futures within the configured bound but `kernel.check_engines_connected()` \
                 returned false; at least one registered NT data or execution client did \
                 not transition to is_connected after NT swallowed/logged its connect error"
            ),
            BoltV3LiveNodeError::DisconnectTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 controlled-disconnect exceeded the configured \
                 nautilus.timeout_disconnection_secs bound ({timeout_secs}s)"
            ),
            BoltV3LiveNodeError::DisconnectFailed(error) => write!(
                f,
                "bolt-v3 controlled-disconnect surfaced an NT engine-level disconnect \
                 aggregator error: {error}"
            ),
            BoltV3LiveNodeError::StrategyFreeStartTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 strategy-free controlled-start exceeded configured \
                 live-node timeout bounds ({timeout_secs}s)"
            ),
            BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow => write!(
                f,
                "bolt-v3 strategy-free controlled-start timeout sum overflowed \
                 config-owned nautilus timeout fields"
            ),
            BoltV3LiveNodeError::StrategyFreeStartIncomplete => write!(
                f,
                "bolt-v3 strategy-free controlled-run exited before NT reached Running \
                 with required startup evidence"
            ),
            BoltV3LiveNodeError::StrategyFreeExecutionAccountsMissing { client_venues } => write!(
                f,
                "bolt-v3 strategy-free controlled-run reached NT Running but required execution \
                 account evidence was absent from NT cache for: {}",
                client_venues.join(", ")
            ),
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(error) => write!(
                f,
                "bolt-v3 strategy-free reference quote probe setup failed: {error}"
            ),
            BoltV3LiveNodeError::StrategyFreeReferenceProbeFailed { reason } => write!(
                f,
                "bolt-v3 strategy-free controlled-run reached NT Running but live reference quote evidence was not observed; engine connectivity cannot be treated as proven: {reason}"
            ),
            BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed { reason } => write!(
                f,
                "bolt-v3 strategy-free controlled-run reached NT Running but data-client readiness evidence was not observed; data-client production readiness cannot be treated as proven: {reason}"
            ),
            BoltV3LiveNodeError::StrategyFreeStartFailed(error) => {
                write!(f, "bolt-v3 strategy-free controlled-start failed: {error}")
            }
            BoltV3LiveNodeError::StrategyFreeStopTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 strategy-free controlled-stop exceeded configured \
                 live-node timeout bounds ({timeout_secs}s)"
            ),
            BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow => write!(
                f,
                "bolt-v3 strategy-free controlled-stop timeout sum overflowed \
                 config-owned nautilus timeout fields"
            ),
            BoltV3LiveNodeError::StrategyFreeStopFailed(error) => {
                write!(f, "bolt-v3 strategy-free controlled-stop failed: {error}")
            }
            BoltV3LiveNodeError::StartupCapitalAdmissionRebuild(decision) => write!(
                f,
                "bolt-v3 startup capital-admission rebuild rejected runtime start: {decision:?}"
            ),
        }
    }
}

impl std::error::Error for BoltV3LiveNodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BoltV3LiveNodeError::ForbiddenEnv(error) => Some(error),
            BoltV3LiveNodeError::SecretResolverSetup(error) => Some(error),
            BoltV3LiveNodeError::SecretResolution(error) => Some(error),
            BoltV3LiveNodeError::AdapterMapping(error) => Some(error),
            BoltV3LiveNodeError::BuilderConstruction(error) => Some(error),
            BoltV3LiveNodeError::ClientRegistration(error) => Some(error),
            BoltV3LiveNodeError::StrategyRegistration(error) => Some(error),
            BoltV3LiveNodeError::RiskPolicy(error) => Some(error.as_ref()),
            BoltV3LiveNodeError::Build(error) => error.source(),
            BoltV3LiveNodeError::KillSwitchStore(error) => Some(error),
            BoltV3LiveNodeError::KillSwitchLossProtection(error) => Some(error.as_ref()),
            BoltV3LiveNodeError::OperatorApprovalConsumption(error) => Some(error.as_ref()),
            BoltV3LiveNodeError::Run(error) => error.source(),
            BoltV3LiveNodeError::RuntimeCaptureWire(error)
            | BoltV3LiveNodeError::RuntimeCaptureShutdown(error) => error.source(),
            BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown { run_error, .. } => {
                Some(run_error.as_ref())
            }
            BoltV3LiveNodeError::DecisionEvidenceShutdownDrain(error) => Some(error.as_ref()),
            BoltV3LiveNodeError::RunAndDecisionEvidenceShutdownDrain { run_error, .. } => {
                Some(run_error.as_ref())
            }
            BoltV3LiveNodeError::ConnectTimeout { .. }
            | BoltV3LiveNodeError::LiveNodeStartupTimeout { .. }
            | BoltV3LiveNodeError::LiveNodeStartupShutdownGraceTimeout { .. }
            | BoltV3LiveNodeError::LiveNodeTraderNotStarted { .. }
            | BoltV3LiveNodeError::ConnectIncomplete
            | BoltV3LiveNodeError::DisconnectTimeout { .. }
            | BoltV3LiveNodeError::LiveTransportScope { .. }
            | BoltV3LiveNodeError::KillSwitchRecovery { .. }
            | BoltV3LiveNodeError::StrategyFreeStartTimeout { .. }
            | BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow
            | BoltV3LiveNodeError::StrategyFreeStartIncomplete
            | BoltV3LiveNodeError::StrategyFreeExecutionAccountsMissing { .. }
            | BoltV3LiveNodeError::StrategyFreeReferenceProbeFailed { .. }
            | BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed { .. }
            | BoltV3LiveNodeError::StrategyFreeStopTimeout { .. }
            | BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow
            | BoltV3LiveNodeError::StartupCapitalAdmissionRebuild(..) => None,
            BoltV3LiveNodeError::DisconnectFailed(error)
            | BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(error)
            | BoltV3LiveNodeError::StrategyFreeStartFailed(error)
            | BoltV3LiveNodeError::StrategyFreeStopFailed(error) => Some(error.as_ref()),
        }
    }
}

fn strategy_free_transport_adapter_configs(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3AdapterConfigs, BoltV3LiveNodeError> {
    map_bolt_v3_adapters(loaded, resolved).map_err(BoltV3LiveNodeError::AdapterMapping)
}

fn strategy_free_transport_loaded_config(loaded: &LoadedBoltV3Config) -> LoadedBoltV3Config {
    let mut strategy_free_loaded = loaded.clone();
    strategy_free_loaded.strategies.clear();
    strategy_free_loaded
}

/// Single bolt-v3 entrypoint for entering NT's runner loop.
///
/// The caller builds the `LiveNode` separately, then this function enters the
/// NT runner loop through the bolt-v3 wrapper that owns runtime capture and
/// shutdown classification. Production callers must use this wrapper rather
/// than invoking the NT runner method directly.
#[derive(Clone, Debug)]
struct LiveNodeStartupWatchdogBounds {
    startup_timeout: Duration,
    shutdown_grace: Duration,
    /// How often to re-check the trader-running launch invariant. Sourced from
    /// config-owned poll intervals (not a business threshold / retry budget).
    trader_invariant_poll: Duration,
    registered_client_labels: Vec<String>,
}

#[derive(Debug)]
enum LiveNodeRunStartupOutcome {
    Finished(Result<(), anyhow::Error>),
    StartupGuardFailed(BoltV3LiveNodeError),
    StartupTimeout {
        timeout_secs: u64,
        node_state: String,
        registered_client_labels: Vec<String>,
    },
    StartupShutdownGraceTimeout {
        trigger: LiveNodeStartupShutdownGraceTrigger,
        shutdown_grace: Duration,
        node_state: String,
        registered_client_labels: Vec<String>,
    },
    /// NT `NodeState::Running` without the trader started — engines-not-connected
    /// fail-open. Must abort launch rather than idle as a trader-less process.
    TraderNotStarted {
        node_state: String,
        registered_client_labels: Vec<String>,
    },
}

/// Pure launch invariant: `NodeState::Running` requires the NT trader to be running.
///
/// NT's engines-not-connected path sets `NodeState::Running` before the trader
/// is started. On the successful path the trader is started before the node
/// transitions to `Running`, so this is not a race — it is the fail-open
/// signature of a trader-less "Running" node.
fn live_node_trader_running_invariant(node_state: NodeState, trader_running: bool) -> bool {
    !matches!(node_state, NodeState::Running) || trader_running
}

fn dispatch_requested_submit_admission_nt_projection(
    node_state: NodeState,
    trigger: Option<&Rc<dyn Fn()>>,
    requested: Option<&Arc<AtomicBool>>,
) -> bool {
    if !matches!(node_state, NodeState::Running) {
        return false;
    }
    let (Some(trigger), Some(requested)) = (trigger, requested) else {
        return false;
    };
    if !requested.swap(false, Ordering::AcqRel) {
        return false;
    }
    trigger();
    true
}

async fn live_node_capture_failure_signal(
    receiver: &mut Option<tokio::sync::oneshot::Receiver<()>>,
) {
    if let Some(receiver) = receiver.as_mut() {
        let _ = receiver.await;
    } else {
        std::future::pending::<()>().await;
    }
}

async fn stop_live_node_startup_with_grace<F, Stop>(
    mut run_future: Pin<&mut F>,
    request_stop: &mut Stop,
    shutdown_grace: Duration,
    reason: &str,
) -> Option<Result<(), anyhow::Error>>
where
    F: Future<Output = Result<(), anyhow::Error>>,
    Stop: FnMut(),
{
    request_stop();
    let startup_shutdown_grace = tokio::time::sleep(shutdown_grace);
    tokio::pin!(startup_shutdown_grace);
    tokio::select! {
        result = run_future.as_mut() => Some(result),
        _ = &mut startup_shutdown_grace => {
            log::error!(
                "LiveNode {reason} shutdown grace elapsed ({shutdown_grace:?})"
            );
            None
        }
    }
}

/// Map a graceful-stop result after a trader-not-started invariant abort.
///
/// When the runner returns within the grace window, the outcome is
/// [`LiveNodeRunStartupOutcome::TraderNotStarted`]. When the grace elapses
/// first, the outcome is still attributed to the trader-not-started invariant
/// via [`LiveNodeStartupShutdownGraceTrigger::TraderNotStartedInvariant`] —
/// never a generic `StartupDeadline` (which would send operators to the wrong
/// failure class).
fn trader_not_started_stop_outcome(
    stop_result: Option<Result<(), anyhow::Error>>,
    shutdown_grace: Duration,
    node_state: String,
    registered_client_labels: Vec<String>,
) -> LiveNodeRunStartupOutcome {
    if let Some(Err(error)) = &stop_result {
        log::error!("LiveNode run failed during trader-not-started shutdown: {error}");
    }
    match stop_result {
        Some(_) => LiveNodeRunStartupOutcome::TraderNotStarted {
            node_state,
            registered_client_labels,
        },
        None => LiveNodeRunStartupOutcome::StartupShutdownGraceTimeout {
            trigger: LiveNodeStartupShutdownGraceTrigger::TraderNotStartedInvariant,
            shutdown_grace,
            node_state,
            registered_client_labels,
        },
    }
}

async fn live_node_run_startup_watchdog<F, State, TraderRunning, Stop, OnRunning>(
    mut run_future: Pin<&mut F>,
    capture_failure_receiver: &mut Option<tokio::sync::oneshot::Receiver<()>>,
    node_state: State,
    trader_running: TraderRunning,
    mut request_stop: Stop,
    mut on_running: OnRunning,
    bounds: LiveNodeStartupWatchdogBounds,
) -> LiveNodeRunStartupOutcome
where
    F: Future<Output = Result<(), anyhow::Error>>,
    State: Fn() -> NodeState,
    TraderRunning: Fn() -> bool,
    Stop: FnMut(),
    OnRunning: FnMut() -> Result<(), BoltV3LiveNodeError>,
{
    let registered_client_labels = bounds.registered_client_labels;
    let startup_deadline = tokio::time::sleep(bounds.startup_timeout);
    tokio::pin!(startup_deadline);
    let mut startup_deadline_fired = false;
    let mut running_guard_completed = false;
    // Poll so a fail-open Running-without-trader launch aborts promptly rather
    // than waiting the full startup timeout. Interval is config-owned; this is
    // detection cadence, not a retry/backoff threshold.
    let mut trader_invariant_poll = tokio::time::interval(bounds.trader_invariant_poll);
    trader_invariant_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        let capture_failure_enabled = capture_failure_receiver.is_some();
        tokio::select! {
            result = run_future.as_mut() => break LiveNodeRunStartupOutcome::Finished(result),
            _ = trader_invariant_poll.tick() => {
                let state = node_state();
                if !live_node_trader_running_invariant(state, trader_running()) {
                    let node_state = format!("{state:?}");
                    log::error!(
                        "LiveNode launch invariant failed: NodeState is Running but trader \
                         is not running (engines-not-connected fail-open); \
                         node_state={node_state}; registered_client_labels={}; requesting stop",
                        live_node_client_list_for_display(&registered_client_labels)
                    );
                    let stop_result = stop_live_node_startup_with_grace(
                        run_future.as_mut(),
                        &mut request_stop,
                        bounds.shutdown_grace,
                        "trader-not-started launch invariant",
                    )
                    .await;
                    break trader_not_started_stop_outcome(
                        stop_result,
                        bounds.shutdown_grace,
                        node_state,
                        registered_client_labels,
                    );
                }
                if matches!(state, NodeState::Running) && !running_guard_completed {
                    if let Err(error) = on_running() {
                        let stop_result = stop_live_node_startup_with_grace(
                            run_future.as_mut(),
                            &mut request_stop,
                            bounds.shutdown_grace,
                            "post-reconciliation capital-admission rebuild",
                        )
                        .await;
                        if stop_result.is_none() {
                            log::error!(
                                "LiveNode post-reconciliation capital-admission rebuild failed \
                                 and shutdown grace elapsed ({:?})",
                                bounds.shutdown_grace
                            );
                        }
                        break LiveNodeRunStartupOutcome::StartupGuardFailed(error);
                    }
                    running_guard_completed = true;
                }
            }
            _ = &mut startup_deadline, if !startup_deadline_fired => {
                startup_deadline_fired = true;
                let state = node_state();
                if !live_node_trader_running_invariant(state, trader_running()) {
                    let node_state = format!("{state:?}");
                    log::error!(
                        "LiveNode startup bound elapsed with Running-but-trader-not-started \
                         fail-open; node_state={node_state}; registered_client_labels={}; \
                         requesting stop",
                        live_node_client_list_for_display(&registered_client_labels)
                    );
                    let stop_result = stop_live_node_startup_with_grace(
                        run_future.as_mut(),
                        &mut request_stop,
                        bounds.shutdown_grace,
                        "trader-not-started launch invariant",
                    )
                    .await;
                    break trader_not_started_stop_outcome(
                        stop_result,
                        bounds.shutdown_grace,
                        node_state,
                        registered_client_labels,
                    );
                } else if matches!(&state, NodeState::Idle | NodeState::Starting) {
                    let node_state = format!("{state:?}");
                    log::error!(
                        "LiveNode startup exceeded configured startup bound \
                         ({:?}); node_state={node_state}; registered_client_labels={}; requesting stop",
                        bounds.startup_timeout,
                        live_node_client_list_for_display(&registered_client_labels)
                    );
                    let stop_result = stop_live_node_startup_with_grace(
                        run_future.as_mut(),
                        &mut request_stop,
                        bounds.shutdown_grace,
                        "startup timeout",
                    )
                    .await;
                    if let Some(Err(error)) = &stop_result {
                        log::error!(
                            "LiveNode run failed during startup timeout shutdown: {error}"
                        );
                    }
                    match stop_result {
                        Some(_) => break LiveNodeRunStartupOutcome::StartupTimeout {
                            timeout_secs: bounds.startup_timeout.as_secs(),
                            node_state,
                            registered_client_labels,
                        },
                        None => break LiveNodeRunStartupOutcome::StartupShutdownGraceTimeout {
                            trigger: LiveNodeStartupShutdownGraceTrigger::StartupDeadline,
                            shutdown_grace: bounds.shutdown_grace,
                            node_state,
                            registered_client_labels,
                        },
                    }
                } else if matches!(&state, NodeState::ShuttingDown | NodeState::Stopped) {
                    let node_state = format!("{state:?}");
                    log::warn!(
                        "LiveNode startup deadline fired while node was already shutting down; \
                         node_state={node_state}; registered_client_labels={}; waiting bounded \
                         startup shutdown grace for runner result",
                        live_node_client_list_for_display(&registered_client_labels)
                    );
                    let stop_result = stop_live_node_startup_with_grace(
                        run_future.as_mut(),
                        &mut request_stop,
                        bounds.shutdown_grace,
                        "startup shutdown",
                    )
                    .await;
                    match stop_result {
                        Some(result) => break LiveNodeRunStartupOutcome::Finished(result),
                        None => break LiveNodeRunStartupOutcome::StartupShutdownGraceTimeout {
                            trigger: LiveNodeStartupShutdownGraceTrigger::StartupDeadline,
                            shutdown_grace: bounds.shutdown_grace,
                            node_state,
                            registered_client_labels,
                        },
                    }
                }
            }
            _ = live_node_capture_failure_signal(capture_failure_receiver), if capture_failure_enabled => {
                let state = node_state();
                if matches!(
                    &state,
                    NodeState::Running | NodeState::ShuttingDown | NodeState::Stopped
                ) {
                    // Running without trader is still the fail-open; abort rather
                    // than treating capture-failure-after-Running as success path.
                    if !live_node_trader_running_invariant(state, trader_running()) {
                        let node_state = format!("{state:?}");
                        log::error!(
                            "NT runtime capture failure on trader-less Running node; \
                             node_state={node_state}; registered_client_labels={}; requesting stop",
                            live_node_client_list_for_display(&registered_client_labels)
                        );
                        let stop_result = stop_live_node_startup_with_grace(
                            run_future.as_mut(),
                            &mut request_stop,
                            bounds.shutdown_grace,
                            "trader-not-started after capture failure",
                        )
                        .await;
                        // Invariant (not capture failure) is the launch cause when the
                        // node is Running without a trader — preserve that attribution
                        // even if shutdown grace elapses.
                        break trader_not_started_stop_outcome(
                            stop_result,
                            bounds.shutdown_grace,
                            node_state,
                            registered_client_labels,
                        );
                    }
                    log::error!(
                        "NT runtime capture failure detected after LiveNode reached Running \
                         or shutdown; awaiting LiveNode shutdown"
                    );
                    break LiveNodeRunStartupOutcome::Finished(run_future.as_mut().await);
                }

                let node_state = format!("{state:?}");
                log::error!(
                    "NT runtime capture failure detected before LiveNode Running; \
                     node_state={node_state}; registered_client_labels={}; requesting stop",
                    live_node_client_list_for_display(&registered_client_labels)
                );
                let stop_result = stop_live_node_startup_with_grace(
                    run_future.as_mut(),
                    &mut request_stop,
                    bounds.shutdown_grace,
                    "startup capture-failure",
                )
                .await;
                match stop_result {
                    Some(result) => break LiveNodeRunStartupOutcome::Finished(result),
                    None => break LiveNodeRunStartupOutcome::StartupShutdownGraceTimeout {
                        trigger: LiveNodeStartupShutdownGraceTrigger::RuntimeCaptureFailure,
                        shutdown_grace: bounds.shutdown_grace,
                        node_state,
                        registered_client_labels,
                    },
                }
            }
        }
    }
}

pub async fn run_bolt_v3_live_node(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    // Wire the durable kill-switch loss protection for the whole run: subscribe
    // the accumulator to position events and spawn its halt-action retry loop.
    // The guard unsubscribes and aborts the retry task on drop.
    let loss_protection_guards = wire_bolt_v3_loss_protection_runtime(runtime);
    runtime.emit_operator_health_surface_transition(OPERATOR_HEALTH_REASON_LIVE_NODE_STARTUP);
    let node_handle = runtime.node.handle();
    let mut capture_guards = {
        let node = &runtime.node;
        wire_bolt_v3_runtime_capture(node, node_handle.clone(), loaded)
    }
    .map_err(BoltV3LiveNodeError::RuntimeCaptureWire)?;
    let mut capture_failure_receiver = capture_guards.take_failure_receiver();
    let iv_start_task = runtime.spawn_iv_engine_start_on_running(&loaded.root)?;
    let startup_timeout_secs = nautilus_startup_bound_secs(&loaded.root.nautilus)
        .map_err(|_| BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow)?;
    let startup_shutdown_grace_secs = live_node_startup_shutdown_grace_secs(loaded)?;
    let startup_client_labels = live_node_startup_client_labels(runtime);
    let reconciliation_cache = runtime.node.kernel().cache();
    let reconciliation_feed = runtime.capital_admission_runtime_feed.clone();
    let reconciliation_evidence = runtime.submit_reservation_recovery.clone();
    let reconciliation_account_ids = runtime
        .submit_admission_nt_reconciliation_account_ids
        .clone();
    let reconciliation_admission = Arc::clone(&runtime.submit_admission);
    let requested_projection_trigger = runtime
        .submit_admission_nt_projection_trigger
        .as_ref()
        .map(Rc::clone);
    let requested_projection_flag = runtime
        .submit_admission_nt_projection_requested
        .as_ref()
        .map(Arc::clone);
    let mut post_reconciliation_guard = move || {
        let observed_at_ns = current_unix_nanos().map_err(BoltV3LiveNodeError::Build)?;
        let decision = BoltV3LiveNodeRuntime::rebuild_capital_admission_from_nt_cache_parts(
            &reconciliation_cache,
            reconciliation_feed.as_ref(),
            reconciliation_evidence.as_ref(),
            &reconciliation_account_ids,
            reconciliation_admission.as_ref(),
            observed_at_ns,
        );
        fail_closed_on_unreconciled_startup_rebuild(decision)
    };

    let run_outcome = {
        let node = &mut runtime.node;
        // Clone the shared trader handle before `node.run()` takes &mut self so the
        // watchdog can probe trader running-state without re-borrowing the node.
        let trader = node.kernel().trader().clone();
        let run_future = node.run();
        tokio::pin!(run_future);
        live_node_run_startup_watchdog(
            run_future.as_mut(),
            &mut capture_failure_receiver,
            || {
                let state = node_handle.state();
                dispatch_requested_submit_admission_nt_projection(
                    state,
                    requested_projection_trigger.as_ref(),
                    requested_projection_flag.as_ref(),
                );
                state
            },
            || trader.borrow().is_running(),
            || node_handle.stop(),
            &mut post_reconciliation_guard,
            LiveNodeStartupWatchdogBounds {
                startup_timeout: Duration::from_secs(startup_timeout_secs),
                shutdown_grace: Duration::from_secs(startup_shutdown_grace_secs),
                trader_invariant_poll: Duration::from_millis(
                    loaded
                        .root
                        .persistence
                        .runtime_capture_start_poll_interval_ms,
                ),
                registered_client_labels: startup_client_labels,
            },
        )
        .await
    };
    if let Some(task) = iv_start_task {
        task.abort();
    }
    let iv_stop_result = runtime.stop_iv_engine_lifecycle(&loaded.root);
    let shutdown_result = capture_guards.shutdown().await;

    let run_and_capture_result = match run_outcome {
        LiveNodeRunStartupOutcome::Finished(run_result) => {
            classify_live_node_run_and_capture_shutdown(run_result, shutdown_result)
        }
        LiveNodeRunStartupOutcome::StartupGuardFailed(error) => {
            if let Err(shutdown_error) = shutdown_result {
                log::error!(
                    "NT runtime capture shutdown failed after startup guard failure: \
                     {shutdown_error}"
                );
            }
            Err(error)
        }
        LiveNodeRunStartupOutcome::StartupTimeout {
            timeout_secs,
            node_state,
            registered_client_labels,
        } => {
            if let Err(error) = shutdown_result {
                log::error!("NT runtime capture shutdown failed after startup timeout: {error}");
            }
            Err(BoltV3LiveNodeError::LiveNodeStartupTimeout {
                timeout_secs,
                node_state,
                registered_client_labels,
            })
        }
        LiveNodeRunStartupOutcome::StartupShutdownGraceTimeout {
            trigger,
            shutdown_grace,
            node_state,
            registered_client_labels,
        } => {
            if let Err(error) = shutdown_result {
                log::error!(
                    "NT runtime capture shutdown failed after startup shutdown grace elapsed: {error}"
                );
            }
            // Preserve the named trader-not-started launch error even when the
            // runner hung through the shutdown grace — operators must not be
            // sent to investigate a generic startup timeout.
            if matches!(
                trigger,
                LiveNodeStartupShutdownGraceTrigger::TraderNotStartedInvariant
            ) {
                Err(BoltV3LiveNodeError::LiveNodeTraderNotStarted {
                    node_state,
                    registered_client_labels,
                })
            } else {
                Err(BoltV3LiveNodeError::LiveNodeStartupShutdownGraceTimeout {
                    trigger,
                    shutdown_grace,
                    node_state,
                    registered_client_labels,
                })
            }
        }
        LiveNodeRunStartupOutcome::TraderNotStarted {
            node_state,
            registered_client_labels,
        } => {
            if let Err(error) = shutdown_result {
                log::error!(
                    "NT runtime capture shutdown failed after trader-not-started abort: {error}"
                );
            }
            Err(BoltV3LiveNodeError::LiveNodeTraderNotStarted {
                node_state,
                registered_client_labels,
            })
        }
    };
    let producer_guards =
        runtime.decision_evidence_producer_guards_for_shutdown(loss_protection_guards);
    let drain_result = runtime
        .drain_decision_evidence_shutdown(producer_guards)
        .await;
    classify_live_node_shutdown(run_and_capture_result, iv_stop_result, drain_result)
}

fn fail_closed_on_unreconciled_startup_rebuild(
    startup_rebuild: BoltV3SubmitCapitalAdmissionRebuildDecision,
) -> Result<(), BoltV3LiveNodeError> {
    if !startup_rebuild.accepted {
        return Err(BoltV3LiveNodeError::StartupCapitalAdmissionRebuild(
            startup_rebuild,
        ));
    }
    Ok(())
}

fn strategy_free_start_timeout_secs(
    loaded: &LoadedBoltV3Config,
) -> Result<u64, BoltV3LiveNodeError> {
    nautilus_startup_bound_secs(&loaded.root.nautilus)
        .map_err(|_| BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow)
}

fn live_node_startup_shutdown_grace_secs(
    loaded: &LoadedBoltV3Config,
) -> Result<u64, BoltV3LiveNodeError> {
    nautilus_stop_budget_secs(loaded)?
        .checked_add(live_node_startup_shutdown_grace_slack_secs(loaded))
        .ok_or(BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow)
}

fn live_node_startup_shutdown_grace_slack_secs(loaded: &LoadedBoltV3Config) -> u64 {
    loaded.root.nautilus.timeout_connection_secs
}

fn live_node_client_list_for_display(clients: &[String]) -> String {
    if clients.is_empty() {
        "none".to_string()
    } else {
        clients.join(", ")
    }
}

fn live_node_startup_client_labels(runtime: &BoltV3LiveNodeRuntime) -> Vec<String> {
    runtime
        .registered_data_client_ids()
        .into_iter()
        .map(|client_id| format!("data:{client_id}"))
        .chain(
            runtime
                .registered_exec_client_ids()
                .into_iter()
                .map(|client_id| format!("exec:{client_id}")),
        )
        .collect()
}

fn live_node_not_connected_client_labels_from_statuses(
    data_client_status: Vec<(ClientId, bool)>,
    exec_client_status: Vec<(ClientId, bool)>,
) -> Vec<String> {
    data_client_status
        .into_iter()
        .filter(|(_, connected)| !*connected)
        .map(|(client_id, _connected)| format!("data:{client_id}"))
        .chain(
            exec_client_status
                .into_iter()
                .filter(|(_, connected)| !*connected)
                .map(|(client_id, _connected)| format!("exec:{client_id}")),
        )
        .collect()
}

fn nautilus_stop_budget_secs(loaded: &LoadedBoltV3Config) -> Result<u64, BoltV3LiveNodeError> {
    loaded
        .root
        .nautilus
        .timeout_disconnection_secs
        .checked_add(loaded.root.nautilus.delay_post_stop_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_shutdown_secs))
        .ok_or(BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow)
}

fn strategy_free_stop_timeout_secs(
    loaded: &LoadedBoltV3Config,
) -> Result<u64, BoltV3LiveNodeError> {
    nautilus_stop_budget_secs(loaded)
}

fn classify_live_node_run_and_capture_shutdown(
    run_result: Result<(), anyhow::Error>,
    shutdown_result: Result<(), anyhow::Error>,
) -> Result<(), BoltV3LiveNodeError> {
    match (run_result, shutdown_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(run_error), Ok(())) => Err(BoltV3LiveNodeError::Run(run_error)),
        (Ok(()), Err(shutdown_error)) => {
            Err(BoltV3LiveNodeError::RuntimeCaptureShutdown(shutdown_error))
        }
        (Err(run_error), Err(shutdown_error)) => {
            log::error!("Live node run error during NT runtime capture shutdown: {run_error}");
            Err(BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown {
                run_error,
                shutdown_error,
            })
        }
    }
}

fn classify_live_node_shutdown(
    run_and_capture_result: Result<(), BoltV3LiveNodeError>,
    iv_stop_result: Result<(), BoltV3LiveNodeError>,
    drain_result: Result<(), BoltV3LiveNodeError>,
) -> Result<(), BoltV3LiveNodeError> {
    let primary_result = match (run_and_capture_result, iv_stop_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(run_or_capture_error), Err(iv_stop_error)) => {
            log::error!("IV lifecycle stop failed after live-node run failure: {iv_stop_error}");
            Err(run_or_capture_error)
        }
    };
    match (primary_result, drain_result) {
        (
            Err(primary_error),
            Err(BoltV3LiveNodeError::DecisionEvidenceShutdownDrain(drain_error)),
        ) => Err(BoltV3LiveNodeError::RunAndDecisionEvidenceShutdownDrain {
            run_error: Box::new(primary_error),
            drain_error,
        }),
        (Err(primary_error), Ok(())) => Err(primary_error),
        (Ok(()), Err(drain_error)) => Err(drain_error),
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary_error), Err(drain_error)) => {
            log::error!(
                "bolt-v3 decision evidence shutdown drain returned unexpected error shape after primary shutdown failure: {drain_error}"
            );
            Err(primary_error)
        }
    }
}

/// Test-friendly builder that lets the caller inject the
/// forbidden-environment predicate and the SSM resolver. Production code
/// resolves the venue secrets once upstream and uses
/// [`build_bolt_v3_live_node_with_resolved`], which applies the real credential
/// environment guard against pre-resolved secrets rather than resolving them
/// again here.
pub fn build_bolt_v3_live_node_with<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let (runtime, _summary) = build_bolt_v3_live_node_with_summary(loaded, env_is_set, resolver)?;
    Ok(runtime)
}

/// Same as [`build_bolt_v3_live_node_with`] but also returns the
/// [`BoltV3RegistrationSummary`] so tests can assert which NT client
/// kinds the registration boundary added before the builder finalized
/// the node. Not intended for production code paths; production reads
/// the summary by other means if it ever needs to.
pub fn build_bolt_v3_live_node_with_summary<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    // RV source-client validation is owned by the strategy-registration
    // chokepoint; trade transport must retain the clients it will validate.
    let transport_loaded =
        trade_transport_loaded_config(loaded, RealizedVolatilityTransportScope::Subscribed)?;
    check_no_forbidden_credential_env_vars_with(&transport_loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let resolved = resolve_bolt_v3_secrets_with(&transport_loaded, resolver)
        .map_err(BoltV3LiveNodeError::SecretResolution)?;
    let bundle =
        live_node_adapter_bundle_with_provider_live_submit_approvals(&transport_loaded, &resolved)?;
    build_live_node_with_clients_and_submit_approval_limits(
        &transport_loaded,
        &resolved,
        bundle.configs,
        bundle.live_submit_approval_limits,
    )
}

pub fn build_bolt_v3_all_configured_client_mapping_live_node_with_summary<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    check_no_forbidden_credential_env_vars_with(&loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let mapping_loaded = strategy_free_transport_loaded_config(loaded);
    validate_trade_transport_execution_venue_cardinality(&mapping_loaded)?;
    let resolved = resolve_bolt_v3_secrets_with(loaded, resolver)
        .map_err(BoltV3LiveNodeError::SecretResolution)?;
    let adapters =
        map_bolt_v3_adapters(loaded, &resolved).map_err(BoltV3LiveNodeError::AdapterMapping)?;
    build_live_node_with_clients(&mapping_loaded, &resolved, adapters)
}

fn build_live_node_with_clients(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    adapters: BoltV3AdapterConfigs,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError> {
    build_live_node_with_clients_and_submit_approval_limits(
        loaded,
        resolved,
        adapters,
        BTreeMap::new(),
    )
}

fn build_live_node_with_clients_and_submit_approval_limits(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    adapters: BoltV3AdapterConfigs,
    live_submit_approval_limits: BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError> {
    let evidence_runtime = DecisionEvidenceRuntime::open(loaded).map_err(|error| {
        BoltV3LiveNodeError::StrategyRegistration(BoltV3StrategyRegistrationError::Evidence {
            message: error.to_string(),
        })
    })?;
    let decision_evidence_status = evidence_runtime.status_view();
    let reservation_recovery = evidence_runtime.reservation_recovery();
    let settlement_recovery = evidence_runtime.settlement_recovery();
    let booking_recovery = evidence_runtime.booking_recovery();
    // Enabled kill-switch boot must fail closed on an unresolved/corrupt/missing
    // durable record before constructing NT clients or registering
    // submit-capable strategy runtime. A clean recovery returns the latched
    // state to seed admission (before registration) and to sync NT trading
    // state (after build).
    let kill_switch_startup_state = recover_kill_switch_state_before_live_node_build(loaded)?;
    let loss_policy = loss_governor_policy_from_loaded(loaded)?;
    let loss_halt_action_policy = loss_governor_halt_action_policy_from_loaded(loaded)?;
    let capital_admission = capital_admission_config_from_loaded(loaded)?;
    validate_live_submit_governance(
        loaded,
        &live_submit_approval_limits,
        loss_policy.is_some(),
        capital_admission.as_ref(),
    )?;
    let iv_client_errors = crate::bolt_v3_validate::validate_iv_source_clients(&loaded.root);
    if !iv_client_errors.is_empty() {
        return Err(BoltV3LiveNodeError::StrategyRegistration(
            BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
                message: format!(
                    "bolt-v3 IV source client validation failed: {}",
                    iv_client_errors.join("; ")
                ),
            },
        ));
    }
    let startup_observed_at_ns = current_unix_nanos().map_err(BoltV3LiveNodeError::Build)?;
    let capital_admission_runtime_feed_config =
        capital_admission_runtime_feed_config_from_loaded(loaded, startup_observed_at_ns);
    let mut submit_admission_nt_reconciliation_account_ids = loaded
        .root
        .risk
        .kill_switch
        .as_ref()
        .filter(|kill_switch| kill_switch.enabled)
        .map(|kill_switch| {
            kill_switch
                .account_ids
                .iter()
                .map(|account_id| AccountId::from(account_id.as_str()))
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    if let Some(config) = capital_admission_runtime_feed_config.as_ref() {
        submit_admission_nt_reconciliation_account_ids.insert(config.account_id);
    }
    let order_reject_observer_account_id = order_reject_observer_account_id_from_loaded(loaded);
    let provider_collateral_allowance_runtime_config =
        provider_collateral_allowance_runtime_config_from_loaded(
            loaded,
            resolved,
            capital_admission_runtime_feed_config.as_ref(),
        )?;
    let submit_reservation_recovery = Arc::clone(&reservation_recovery);
    let submit_admission = Arc::new(
        BoltV3SubmitAdmissionState::new_with_live_submit_limits_and_optional_controls(
            evidence_runtime.submit_admission_evidence(),
            live_submit_approval_limits,
            loss_policy.clone(),
            capital_admission,
        ),
    );
    submit_admission.install_recovered_admission_authority(Arc::clone(&reservation_recovery));
    // Latch the recovered kill-switch state into submit admission before any
    // submit-capable strategy runtime is registered, so a recovered halt blocks
    // submits from the first registered strategy onward.
    if let Some(state) = kill_switch_startup_state.as_ref() {
        submit_admission.replace_kill_switch_state(state.clone());
    }
    let capital_admission_runtime_feed = capital_admission_runtime_feed_config.map(|config| {
        Arc::new(Mutex::new(CapitalAdmissionRuntimeFeed::new(
            config,
            submit_admission.clone(),
        )))
    });
    let order_reject_observer_feed = order_reject_observer_account_id.map(|account_id| {
        Arc::new(Mutex::new(BoltV3OrderRejectObserverFeed::new(
            evidence_runtime.order_reject_observer_evidence(),
            account_id,
        )))
    });
    let operator_health_transition_logger = BoltV3OperatorHealthTransitionLogger::new();
    let settlement_health = Arc::new(Mutex::new(settlement_health_from_loaded(loaded)));
    let input_health_sources_by_client =
        reference_current_price_live_input_sources_by_client(loaded);
    let input_health_configured_source_count =
        configured_reference_current_price_source_count(&input_health_sources_by_client);
    let input_health_accumulator = Arc::new(Mutex::new(BoltV3LiveInputHealthAccumulator::new(
        input_health_configured_source_count,
        &input_health_sources_by_client,
    )));
    let emit_operator_health_surface: Arc<
        dyn Fn(&'static str, Option<BoltV3InputHealth>) -> Result<()> + Send + Sync + 'static,
    > = {
        let order_reject_observer_feed = order_reject_observer_feed.clone();
        let submit_admission = submit_admission.clone();
        let logger = operator_health_transition_logger.clone();
        let settlement_health = settlement_health.clone();
        let decision_evidence = decision_evidence_status.clone();
        let provider_collateral_allowance_configured = capital_admission_runtime_feed.is_some();
        Arc::new(move |reason, input_health| {
            let settlement_health = settlement_health_snapshot(&settlement_health)?;
            let surface = live_operator_health_surface(
                order_reject_observer_feed.as_ref(),
                &submit_admission,
                provider_collateral_allowance_configured,
                input_health_configured_source_count,
                input_health,
                settlement_health,
                &decision_evidence,
            );
            logger.emit_surface(reason, surface);
            Ok(())
        })
    };
    let operator_health_transition_emitter: BoltV3OperatorHealthTransitionEmitter = {
        let emit_operator_health_surface = emit_operator_health_surface.clone();
        let input_health_accumulator = input_health_accumulator.clone();
        Arc::new(move |reason| {
            let input_health = live_input_health_snapshot(&input_health_accumulator);
            if let Err(error) = emit_operator_health_surface(reason, input_health) {
                log::error!(
                    "operator health surface transition failed: reason={reason} error={error:#}"
                );
            }
        })
    };
    {
        let emit_operator_health_surface = emit_operator_health_surface.clone();
        evidence_runtime
            .register_health_transition_publisher(Arc::new(move |transition| {
                let reason = transition.operator_health_reason();
                if let Err(error) = emit_operator_health_surface(reason, None) {
                    log::error!(
                        "decision-evidence health transition failed: reason={reason} error={error:#}"
                    );
                }
            }))
            .map_err(|message| {
                BoltV3LiveNodeError::StrategyRegistration(
                    BoltV3StrategyRegistrationError::Evidence {
                        message: message.to_string(),
                    },
                )
            })?;
    }
    let input_health_transition_emitter: BoltV3InputHealthTransitionEmitter = {
        let emit_operator_health_surface = emit_operator_health_surface.clone();
        let input_health_accumulator = input_health_accumulator.clone();
        Arc::new(move |reason, transition| {
            let input_health = apply_live_input_health_transition(
                &input_health_accumulator,
                input_health_configured_source_count,
                transition,
            );
            if let Err(error) = emit_operator_health_surface(reason, Some(input_health)) {
                log::error!(
                    "input health surface transition failed: reason={reason} error={error:#}"
                );
            }
        })
    };
    let settlement_health_transition_emitter = build_settlement_health_transition_emitter(
        settlement_health.clone(),
        input_health_accumulator.clone(),
        emit_operator_health_surface.clone(),
    );
    let order_reject_observer_feed_subscription = order_reject_observer_feed.as_ref().map(|feed| {
        subscribe_order_reject_observer_feed_with_health_emitter(
            feed.clone(),
            operator_health_transition_emitter.clone(),
        )
    });
    let order_execution_policy =
        crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::from_mode(
            loaded.root.runtime.order_execution_mode,
        );
    let builder =
        make_bolt_v3_live_node_builder(loaded).map_err(BoltV3LiveNodeError::BuilderConstruction)?;
    let mut adapters = adapters;
    bolt_v3_providers::attach_live_input_health_transition_emitters(
        &mut adapters,
        input_health_transition_emitter,
        &input_health_sources_by_client,
    );
    let (builder, summary) = register_bolt_v3_clients(builder, adapters)
        .map_err(BoltV3LiveNodeError::ClientRegistration)?;
    let mut node = builder.build().map_err(BoltV3LiveNodeError::Build)?;
    let submit_admission_nt_projection_requested =
        (!submit_admission_nt_reconciliation_account_ids.is_empty())
            .then(|| Arc::new(AtomicBool::new(false)));
    let (submit_admission_nt_projection_subscription, submit_admission_nt_projection_trigger) =
        match submit_admission_nt_projection_requested.as_ref() {
            Some(projection_requested) => {
                let projection_cache = node.kernel().cache();
                let projection_feed = capital_admission_runtime_feed.clone();
                let projection_recovery = submit_reservation_recovery.clone();
                let projection_account_ids = submit_admission_nt_reconciliation_account_ids.clone();
                let projection_admission = Arc::clone(&submit_admission);
                let projection_health_emitter = operator_health_transition_emitter.clone();
                let projection_requested = Arc::clone(projection_requested);
                let projection_request: Rc<dyn Fn()> = Rc::new(move || {
                    projection_requested.store(true, Ordering::Release);
                });
                let projection_trigger: Rc<dyn Fn()> = Rc::new(move || {
                    let observed_at_ns = match current_unix_nanos() {
                        Ok(observed_at_ns) => observed_at_ns,
                        Err(error) => {
                            log::error!(
                                "capital-admission NT projection clock failed after canonical NT event: {error:#}"
                            );
                            return;
                        }
                    };
                    let decision =
                        BoltV3LiveNodeRuntime::rebuild_capital_admission_from_nt_cache_parts(
                            &projection_cache,
                            projection_feed.as_ref(),
                            projection_recovery.as_ref(),
                            &projection_account_ids,
                            projection_admission.as_ref(),
                            observed_at_ns,
                        );
                    if !decision.accepted {
                        log::warn!(
                            "capital-admission NT projection remained unreconciled after canonical NT event: {:?}",
                            decision.reason
                        );
                    }
                    projection_health_emitter(
                        OPERATOR_HEALTH_REASON_SUBMIT_ADMISSION_NT_PROJECTION,
                    );
                });
                let subscription = subscribe_submit_admission_nt_projection(
                    capital_admission_runtime_feed.clone(),
                    projection_request,
                );
                (Some(subscription), Some(projection_trigger))
            }
            None => (None, None),
        };
    // Sync the recovered kill-switch state into NT's RiskEngine trading state so
    // the NT risk engine and the submit-admission latch agree on the halt. The
    // loss-protection seed below can override this for fail-closed cases.
    if let Some(state) = kill_switch_startup_state.as_ref() {
        sync_nt_trading_state_for_kill_switch(&mut node, state);
    }
    let node_scoped_source_announcements = node_scoped_runtime_source_announcements(
        loaded,
        provider_collateral_allowance_runtime_config.is_some(),
    );
    if let Some(announcement) =
        &node_scoped_source_announcements.provider_collateral_allowance_rest_capture
    {
        log::warn!(
            "bolt-v3 runtime feed announcement: {}",
            serde_json::to_string(announcement)
                .expect("node-scoped runtime source announcement should serialize")
        );
    }
    for announcement in &node_scoped_source_announcements.iv_runtime_sources {
        log::warn!(
            "bolt-v3 runtime feed announcement: {}",
            serde_json::to_string(announcement)
                .expect("node-scoped runtime source announcement should serialize")
        );
    }
    let settlement_loss_protection_slot: BoltV3LiveSettlementLossProtectionSlot =
        Rc::new(RefCell::new(None));
    let settlement_runtime_sink_backends =
        BoltV3SettlementRuntimeSinkBackends::from_root(&loaded.root);
    let settlement_runtime_sink = settlement_runtime_sink_handle(
        settlement_runtime_sink_backends
            .loss_protection()
            .then(|| settlement_loss_protection_slot.clone()),
    );
    debug_assert_eq!(
        settlement_runtime_sink.is_some(),
        settlement_runtime_sink_backends.will_configure_runtime_sink()
    );
    let settlement_recovery = Some(Arc::clone(&settlement_recovery));
    let booking_recovery = Some(Arc::clone(&booking_recovery));
    let strategy_execution_controls = BoltV3StrategyExecutionControls {
        submit_admission: submit_admission.clone(),
        order_execution_policy,
        settlement_runtime_sink,
        settlement_recovery,
        booking_recovery,
        settlement_health_transition_emitter: Some(settlement_health_transition_emitter),
    };
    let iv_runtime = loaded
        .root
        .iv
        .as_ref()
        .map(IvRuntimeEngine::from_iv_root)
        .transpose()
        .map_err(|error| {
            BoltV3LiveNodeError::StrategyRegistration(
                BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
                    message: format!("bolt-v3 IV runtime engine construction failed: {error:?}"),
                },
            )
        })?;
    if let Some(iv_runtime) = &iv_runtime {
        let lifecycle = plan_iv_engine_lifecycle(&loaded.root).map_err(|error| {
            BoltV3LiveNodeError::StrategyRegistration(
                BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
                    message: format!("bolt-v3 IV lifecycle planning failed: {error:?}"),
                },
            )
        })?;
        let outcomes = {
            let mut adapter = NtIvRuntimePlanValidationAdapter::new(
                &node,
                &loaded.root.nautilus.data_engine.external_clients,
            );
            apply_subscription_plans(&mut adapter, &lifecycle.start_plans)
        };
        iv_runtime.apply_plan_outcomes(&outcomes).map_err(|error| {
            BoltV3LiveNodeError::StrategyRegistration(
                BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
                    message: format!("bolt-v3 IV lifecycle state update failed: {error:?}"),
                },
            )
        })?;
    }
    let iv_event_bindings =
        if let (Some(iv), Some(iv_runtime)) = (loaded.root.iv.as_ref(), iv_runtime.as_ref()) {
            Some(
                wire_bolt_v3_iv_runtime_event_bindings(iv, iv_runtime)
                    .map_err(BoltV3LiveNodeError::StrategyRegistration)?,
            )
        } else {
            None
        };
    let strategy_summary = if let Some(iv_runtime) = &iv_runtime {
        register_bolt_v3_strategies_on_node_with_iv_runtime_bindings(
            &mut node,
            loaded,
            resolved,
            crate::strategy_bindings::production_runtime_bindings(),
            strategy_execution_controls,
            evidence_runtime.strategy_evidence_handles(),
            iv_runtime,
        )
    } else {
        register_bolt_v3_strategies_on_node_with_bindings(
            &mut node,
            loaded,
            resolved,
            crate::strategy_bindings::production_runtime_bindings(),
            strategy_execution_controls,
            evidence_runtime.strategy_evidence_handles(),
        )
    }
    .map_err(BoltV3LiveNodeError::StrategyRegistration)?;
    let source_announcements =
        runtime_source_announcements(loaded, &strategy_summary).map_err(|message| {
            BoltV3LiveNodeError::StrategyRegistration(BoltV3StrategyRegistrationError::Evidence {
                message,
            })
        })?;
    for strategy in &strategy_summary.registered {
        log::info!(
            "bolt-v3 registered strategy: strategy_instance_id={} strategy_archetype={} nt_strategy_id={}",
            strategy.strategy_instance_id,
            strategy.strategy_archetype.as_str(),
            strategy.registered_strategy_id
        );
    }
    for announcement in &source_announcements {
        log::warn!(
            "bolt-v3 runtime feed announcement: {}",
            serde_json::to_string(announcement)
                .expect("runtime source announcement should serialize")
        );
    }
    let provider_collateral_allowance_runtime_guard = match (
        provider_collateral_allowance_runtime_config,
        capital_admission_runtime_feed.as_ref(),
    ) {
        (Some(config), Some(feed)) => Some(spawn_provider_collateral_allowance_runtime(
            config,
            feed.clone(),
            submit_admission.clone(),
            node.handle(),
            Some(operator_health_transition_emitter.clone()),
            submit_admission_nt_projection_requested
                .as_ref()
                .expect("capital admission feed should own a projection request signal")
                .clone(),
        )),
        (None, _) => None,
        (Some(_), None) => {
            return Err(BoltV3LiveNodeError::Build(anyhow::anyhow!(
                "provider collateral allowance runtime requires the capital admission runtime feed"
            )));
        }
    };
    // Configure the durable kill-switch loss-protection accumulator after
    // strategies are registered (its flatten targets are the registered NT
    // strategy ids) and seed it from the durable store. `seed_from_store` can
    // fail closed (e.g. an armed durable record with no loss snapshot becomes
    // `FailedManualIntervention`) and override the kill-switch state established
    // above by `recover_kill_switch_state_before_live_node_build`, so re-sync NT
    // trading state from the final loss-protection state — otherwise a
    // fail-closed seed would latch admission while leaving NT trading `Active`.
    let loss_protection = configure_bolt_v3_kill_switch_loss_protection(
        loaded,
        &node,
        evidence_runtime.order_execution_evidence(),
        submit_admission.clone(),
    )?;
    debug_assert!(
        !settlement_runtime_sink_backends.loss_protection() || loss_protection.is_some(),
        "kill-switch settlement sink backend must match loss-protection construction"
    );
    if let Some(protection) = loss_protection.as_ref() {
        *settlement_loss_protection_slot.borrow_mut() = Some(protection.clone());
        let seeded_state = protection.borrow().state().clone();
        sync_nt_trading_state_for_kill_switch(&mut node, &seeded_state);
    }
    let loss_halt_action_handler =
        match (loss_policy.clone(), loss_halt_action_policy.as_ref()) {
            (Some(policy), Some(action_policy)) => Some(
                loss_governor_halt_action_handler_from_node(&node, policy, *action_policy),
            ),
            _ => None,
        };
    let (loss_runtime_feed, loss_runtime_feed_subscription) =
        match loss_governor_runtime_feed_config_from_loaded(loaded)? {
            Some(config) => {
                let feed = LossGovernorRuntimeFeed::new(config, submit_admission.clone());
                let feed = match loss_halt_action_handler.as_ref() {
                    Some(handler) => feed.with_halt_action_handler(handler.clone()),
                    None => feed,
                };
                let feed = Rc::new(RefCell::new(feed));
                let subscription = subscribe_loss_governor_runtime_feed(feed.clone());
                (Some(feed), Some(subscription))
            }
            None => (None, None),
        };
    let runtime = BoltV3LiveNodeRuntime::new(
        node,
        summary.clone(),
        submit_admission,
        BoltV3LiveNodeRuntimeFeeds {
            loss_protection,
            loss_halt_action_policy,
            loss_runtime_feed,
            loss_runtime_feed_subscription,
            order_reject_observer_feed,
            order_reject_observer_feed_subscription,
            capital_admission_runtime_feed,
            submit_admission_nt_projection_subscription,
            submit_admission_nt_projection_trigger,
            submit_admission_nt_projection_requested,
            provider_collateral_allowance_runtime_guard,
            submit_reservation_recovery,
            submit_admission_nt_reconciliation_account_ids,
        },
        BoltV3LiveNodeRuntimeComponents {
            iv_runtime,
            iv_event_bindings,
            operator_health_transition_logger,
            input_health_configured_source_count,
            settlement_health,
            decision_evidence_runtime: evidence_runtime,
            decision_evidence: decision_evidence_status,
            redaction_values: resolved.redaction_values(),
        },
    );
    Ok((runtime, summary))
}

fn validate_live_submit_governance(
    loaded: &LoadedBoltV3Config,
    live_submit_approval_limits: &BTreeMap<String, BoltV3LiveSubmitApprovalLimits>,
    loss_policy_present: bool,
    capital_admission: Option<&BoltV3SubmitCapitalAdmissionConfig>,
) -> Result<(), BoltV3LiveNodeError> {
    if loaded.strategies.is_empty() || explicit_live_submit_governance_declaration(loaded) {
        return Ok(());
    }

    let uncovered_execution_client_ids = submit_capable_execution_client_ids(loaded)
        .into_iter()
        .filter(|execution_client_id| {
            !live_submit_approval_limits.contains_key(execution_client_id.as_str())
                && !loss_governor_covers_execution_client(
                    loaded,
                    execution_client_id,
                    loss_policy_present,
                )
                && !capital_admission_covers_execution_client(
                    loaded,
                    execution_client_id,
                    capital_admission,
                )
        })
        .collect::<Vec<_>>();

    if uncovered_execution_client_ids.is_empty() {
        return Ok(());
    }

    Err(BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
        "submit-capable live node has uncovered execution_client_id(s) {}; each submit-capable \
         execution client must be covered by capital admission, a live-submit approval limits entry \
         keyed to that execution_client_id, or a loss policy; otherwise declare \
         risk.live_submit_governance.mode = \"supervised_deposit_capped\" for supervised \
         deposit-capped operation",
        uncovered_execution_client_ids.join(", ")
    )))
}

fn explicit_live_submit_governance_declaration(loaded: &LoadedBoltV3Config) -> bool {
    matches!(
        loaded
            .root
            .risk
            .live_submit_governance
            .as_ref()
            .map(|governance| governance.mode),
        Some(LiveSubmitGovernanceMode::SupervisedDepositCapped)
    )
}

fn submit_capable_execution_client_ids(loaded: &LoadedBoltV3Config) -> BTreeSet<String> {
    loaded
        .strategies
        .iter()
        .filter_map(|strategy| {
            let execution_client_id = strategy.config.execution_client_id.as_str();
            let client = loaded.root.clients.get(execution_client_id)?;
            client
                .execution
                .is_some()
                .then(|| execution_client_id.to_string())
        })
        .collect()
}

fn loss_governor_covers_execution_client(
    loaded: &LoadedBoltV3Config,
    execution_client_id: &str,
    loss_policy_present: bool,
) -> bool {
    if !loss_policy_present {
        return false;
    }
    let Some(loss_governor) = loaded.root.risk.loss_governor.as_ref() else {
        return false;
    };
    if !loss_governor.enabled {
        return false;
    }
    let loss_governor_account_id = loss_governor.account_id.to_string();
    execution_client_account_id(loaded, execution_client_id)
        .is_some_and(|account_id| account_id == loss_governor_account_id)
}

fn capital_admission_covers_execution_client(
    loaded: &LoadedBoltV3Config,
    execution_client_id: &str,
    capital_admission: Option<&BoltV3SubmitCapitalAdmissionConfig>,
) -> bool {
    let Some(capital_admission) = capital_admission else {
        return false;
    };
    let Some(client) = loaded.root.clients.get(execution_client_id) else {
        return false;
    };
    if client.venue.as_str() != capital_admission.venue_id.as_str() {
        return false;
    }
    execution_account_id(client)
        .is_some_and(|account_id| account_id == capital_admission.account_id.as_str())
}

fn execution_client_account_id<'a>(
    loaded: &'a LoadedBoltV3Config,
    execution_client_id: &str,
) -> Option<&'a str> {
    let client = loaded.root.clients.get(execution_client_id)?;
    execution_account_id(client)
}

fn execution_account_id(client: &ClientBlock) -> Option<&str> {
    client
        .execution
        .as_ref()?
        .as_table()?
        .get(stringify!(account_id))?
        .as_str()
}

#[cfg(test)]
mod tests;
