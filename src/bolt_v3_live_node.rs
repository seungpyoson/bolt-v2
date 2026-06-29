//! Bolt-v3 NautilusTrader LiveNode assembly without strategy registration,
//! market selection, order construction, or submit paths.
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
    path::PathBuf,
    rc::Rc,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ahash::AHashMap;
use anyhow::Result;
use log::LevelFilter;
use nautilus_common::{
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
        CapitalAdmissionRuntimeFeed, CapitalAdmissionRuntimeFeedConfig,
        CapitalAdmissionRuntimeFeedSubscription, subscribe_capital_admission_runtime_feed,
    },
    bolt_v3_capital_admission_state::{
        VenueSpendabilityIdentity, VenueSpendabilitySnapshot, VenueSpendabilitySourceFileRequest,
        venue_spendability_snapshot_from_json_file,
    },
    bolt_v3_capital_reservation::CapitalPoolSnapshot,
    bolt_v3_client_registration::{
        BoltV3ClientRegistrationError, BoltV3RegistrationSummary, register_bolt_v3_clients,
    },
    bolt_v3_config::{
        BoltV3RootConfig, CapitalPoolBlock, DataClientReadinessProbeBlock,
        DataClientReadinessProbeBookType, DataClientReadinessProbeMarketDataKind,
        DataClientReadinessProbeQuoteTargetSource, LoadedBoltV3Config, LoadedStrategy,
        resolve_root_relative_path,
    },
    bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3BasketAdmissionDecisionEvidence,
        BoltV3CapitalAdmissionRebuildAuditEvidence, BoltV3DecisionEvidenceWriter,
        BoltV3EntrySkipEvidence, BoltV3ExitDecisionEvidence, BoltV3ExitEvaluationEvidence,
        BoltV3LossGovernorHaltEvidence, BoltV3OrderIntentEvidence, BoltV3OrderRejectEvidence,
        BoltV3RequoteThrottleEvidence, BoltV3StrategyInputEvidenceSnapshot,
        BoltV3SubmitReservationFillEvidence, BoltV3SubmitReservationMetadataEvidence,
        JsonlBoltV3DecisionEvidenceWriter, decision_evidence_path,
        read_submit_reservation_recovery_evidence,
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
    bolt_v3_kill_switch::{KillSwitchState, KillSwitchStateKind},
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
        KillSwitchLossProtection, KillSwitchLossProtectionConfig,
    },
    bolt_v3_loss_runtime_feed::{
        LossGovernorRuntimeFeed, LossGovernorRuntimeFeedConfig,
        LossGovernorRuntimeFeedSubscription, subscribe_loss_governor_runtime_feed,
    },
    bolt_v3_order_reject_observer_feed::{
        BoltV3OrderRejectObserverFeed, OrderRejectObserverFeedSubscription,
        subscribe_order_reject_observer_feed,
    },
    bolt_v3_providers::{
        self, ProviderLiveSubmitApprovalContext, ProviderLiveSubmitApprovals,
        ProviderRuntimeApprovals,
    },
    bolt_v3_reference_price::reference_price_source_is_runtime_available,
    bolt_v3_secrets::{
        BoltV3SecretError, ForbiddenEnvVarError, ResolvedBoltV3Secrets,
        check_no_forbidden_credential_env_vars, check_no_forbidden_credential_env_vars_with,
        resolve_bolt_v3_secrets, resolve_bolt_v3_secrets_with,
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
        BoltV3SubmitCapitalAdmissionNtComponents, BoltV3SubmitCapitalAdmissionOpenOrderEvidence,
        BoltV3SubmitCapitalAdmissionOpenOrderSnapshot, BoltV3SubmitCapitalAdmissionRebuildDecision,
    },
    bolt_v3_validate::parse_decimal_string,
    nt_runtime_capture::{
        NtRuntimeCaptureGuards, position_events_pattern, wire_nt_runtime_capture,
    },
    secrets::SsmResolverSession,
};

mod live_node_config;
mod transport_scope;

#[cfg(test)]
use live_node_config::make_bolt_v3_live_node_builder_from_config;
pub use live_node_config::{
    connect_bolt_v3_clients, disconnect_bolt_v3_clients, make_bolt_v3_live_node_builder,
    make_live_node_config, wire_bolt_v3_runtime_capture,
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
    capital_admission_runtime_feed_subscription: Option<CapitalAdmissionRuntimeFeedSubscription>,
    capital_admission_venue_spendability_source:
        Option<BoltV3CapitalAdmissionVenueSpendabilitySourceConfig>,
    submit_reservation_recovery: Option<BoltV3SubmitReservationRecoveryConfig>,
    iv_runtime: Option<IvRuntimeEngine>,
    iv_event_bindings: Option<BoltV3IvRuntimeEventBindings>,
    redaction_values: Vec<Zeroizing<String>>,
}

#[derive(Debug, Clone)]
struct BoltV3CapitalAdmissionVenueSpendabilitySourceConfig {
    path: PathBuf,
    max_bytes: u64,
    expected_sha256: String,
    venue_id: String,
    account_id: String,
    collateral_currency: String,
}

/// Startup reservation-recovery source: the decision-evidence file the
/// live-node boot driver reads to recover known submit-reservation
/// metadata after a restart, plus the byte cap from
/// [`crate::bolt_v3_config::DecisionEvidenceBlock::recovery_evidence_max_bytes`].
#[derive(Debug, Clone)]
struct BoltV3SubmitReservationRecoveryConfig {
    path: PathBuf,
    max_bytes: u64,
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

#[derive(Debug, Clone, PartialEq)]
pub struct IvEngineLifecyclePlan {
    pub start_plans: Vec<IvSubscriptionPlan>,
    pub reload_plans: Vec<IvSubscriptionPlan>,
    pub stop_plans: Vec<IvSubscriptionPlan>,
}

pub fn plan_iv_engine_lifecycle(
    root: &BoltV3RootConfig,
) -> Result<IvEngineLifecyclePlan, IvSubscriptionError> {
    let Some(iv) = &root.iv else {
        return Ok(IvEngineLifecyclePlan {
            start_plans: Vec::new(),
            reload_plans: Vec::new(),
            stop_plans: Vec::new(),
        });
    };

    let mut start_plans = Vec::new();
    let reload_plans = Vec::new();
    let mut stop_plans = Vec::new();
    for profile in &iv.profiles {
        let subscription_config = profile.subscription_config();
        start_plans.extend(plan_profile_start(&subscription_config)?);
        stop_plans.extend(plan_profile_stop(&subscription_config)?);
    }

    Ok(IvEngineLifecyclePlan {
        start_plans,
        reload_plans,
        stop_plans,
    })
}

pub fn plan_iv_engine_reload_lifecycle(
    current_root: &BoltV3RootConfig,
    next_root: &BoltV3RootConfig,
) -> Result<IvEngineLifecyclePlan, IvSubscriptionError> {
    let current_profiles = current_root.iv.as_ref().map(|iv| &iv.profiles);
    let next_profiles = next_root.iv.as_ref().map(|iv| &iv.profiles);
    let mut start_plans = Vec::new();
    let mut reload_plans = Vec::new();
    let mut stop_plans = Vec::new();

    match (current_profiles, next_profiles) {
        (None, None) => {}
        (None, Some(next_profiles)) => {
            for profile in next_profiles {
                start_plans.extend(plan_profile_start(&profile.subscription_config())?);
            }
        }
        (Some(current_profiles), None) => {
            for profile in current_profiles {
                stop_plans.extend(plan_profile_stop(&profile.subscription_config())?);
            }
        }
        (Some(current_profiles), Some(next_profiles)) => {
            let current_by_id = current_profiles
                .iter()
                .map(|profile| (&profile.profile_id, profile))
                .collect::<BTreeMap<_, _>>();
            let next_by_id = next_profiles
                .iter()
                .map(|profile| (&profile.profile_id, profile))
                .collect::<BTreeMap<_, _>>();

            for current_profile in current_profiles {
                if let Some(next_profile) = next_by_id.get(&current_profile.profile_id) {
                    reload_plans.extend(plan_profile_reload(
                        &current_profile.subscription_config(),
                        &next_profile.subscription_config(),
                    )?);
                } else {
                    stop_plans.extend(plan_profile_stop(&current_profile.subscription_config())?);
                }
            }

            for next_profile in next_profiles {
                if !current_by_id.contains_key(&next_profile.profile_id) {
                    start_plans.extend(plan_profile_start(&next_profile.subscription_config())?);
                }
            }
        }
    }

    Ok(IvEngineLifecyclePlan {
        start_plans,
        reload_plans,
        stop_plans,
    })
}

pub struct BoltV3IvRuntimeEventBindings {
    option_greeks: Vec<BoltV3IvOptionGreeksRuntimeEventBinding>,
    option_chains: Vec<BoltV3IvOptionChainRuntimeEventBinding>,
    custom_data: Vec<BoltV3IvCustomDataRuntimeEventBinding>,
}

struct BoltV3IvOptionGreeksRuntimeEventBinding {
    pattern: MStr<Pattern>,
    handler: TypedHandler<OptionGreeks>,
}

struct BoltV3IvOptionChainRuntimeEventBinding {
    pattern: MStr<Pattern>,
    handler: TypedHandler<OptionChainSlice>,
}

struct BoltV3IvCustomDataRuntimeEventBinding {
    pattern: MStr<Pattern>,
    handler: ShareableMessageHandler,
}

impl Drop for BoltV3IvRuntimeEventBindings {
    fn drop(&mut self) {
        for binding in self.option_greeks.drain(..) {
            msgbus::unsubscribe_option_greeks(binding.pattern, &binding.handler);
        }
        for binding in self.option_chains.drain(..) {
            msgbus::unsubscribe_option_chain(binding.pattern, &binding.handler);
        }
        for binding in self.custom_data.drain(..) {
            msgbus::unsubscribe_any(binding.pattern, &binding.handler);
        }
    }
}

pub fn wire_bolt_v3_iv_runtime_event_bindings(
    iv: &IvRootConfig,
    runtime: &IvRuntimeEngine,
) -> Result<BoltV3IvRuntimeEventBindings, BoltV3StrategyRegistrationError> {
    let mut bindings = BoltV3IvRuntimeEventBindings {
        option_greeks: Vec::new(),
        option_chains: Vec::new(),
        custom_data: Vec::new(),
    };

    for profile in &iv.profiles {
        for source in &profile.sources {
            match (&source.source_kind, &source.selector) {
                (
                    IvSourceKind::OptionGreeks,
                    IvSelector::SourceOptionGreeks { instrument_ids, .. },
                ) => {
                    let instrument_ids = parse_option_greeks_instrument_ids(instrument_ids)
                        .map_err(|message| {
                            iv_runtime_event_binding_error(
                                &profile.profile_id,
                                &source.source_id,
                                message,
                            )
                        })?;
                    for instrument_id in instrument_ids {
                        bindings
                            .option_greeks
                            .push(wire_option_greeks_event_binding(
                                &profile.profile_id,
                                &source.source_id,
                                instrument_id,
                                runtime,
                            ));
                    }
                }
                (IvSourceKind::OptionChain, IvSelector::SourceOptionChain { series_ids, .. }) => {
                    let series_ids =
                        parse_option_chain_series_ids(series_ids).map_err(|message| {
                            iv_runtime_event_binding_error(
                                &profile.profile_id,
                                &source.source_id,
                                message,
                            )
                        })?;
                    for series_id in series_ids {
                        bindings.option_chains.push(wire_option_chain_event_binding(
                            &profile.profile_id,
                            &source.source_id,
                            series_id,
                            runtime,
                        ));
                    }
                }
                (
                    IvSourceKind::AggregateGreeks,
                    IvSelector::SourceAggregateGreeks {
                        aggregate_key,
                        underlying_selectors,
                        nt_params,
                        ..
                    },
                ) => {
                    let (data_type, _) = aggregate_greeks_data_type_for_source(
                        &source.source_id,
                        aggregate_key,
                        underlying_selectors,
                        &source.params,
                        nt_params,
                    )
                    .map_err(|message| {
                        iv_runtime_event_binding_error(
                            &profile.profile_id,
                            &source.source_id,
                            message,
                        )
                    })?;
                    bindings
                        .custom_data
                        .push(wire_aggregate_greeks_custom_data_event_binding(
                            &profile.profile_id,
                            &source.source_id,
                            data_type,
                            runtime,
                        ));
                }
                (
                    IvSourceKind::CustomImpliedVolatility,
                    IvSelector::SourceCustomImpliedVolatility {
                        custom_iv_data_type,
                        nt_params,
                        ..
                    },
                ) => {
                    let (data_type, _) = custom_iv_data_type_for_source(
                        &source.source_id,
                        custom_iv_data_type,
                        &source.params,
                        nt_params,
                    )
                    .map_err(|message| {
                        iv_runtime_event_binding_error(
                            &profile.profile_id,
                            &source.source_id,
                            message,
                        )
                    })?;
                    bindings.custom_data.push(wire_custom_iv_event_binding(
                        &profile.profile_id,
                        &source.source_id,
                        data_type,
                        runtime,
                    ));
                }
                _ => {}
            }
        }
    }

    Ok(bindings)
}

fn parse_option_greeks_instrument_ids(
    instrument_ids: &[String],
) -> Result<Vec<InstrumentId>, String> {
    instrument_ids
        .iter()
        .map(|instrument_id| {
            InstrumentId::from_str(instrument_id).map_err(|error| {
                format!("invalid NT option-greeks instrument_id {instrument_id}: {error}")
            })
        })
        .collect()
}

fn parse_option_chain_series_ids(series_ids: &[String]) -> Result<Vec<OptionSeriesId>, String> {
    series_ids
        .iter()
        .map(|series_id| {
            OptionSeriesId::from_str(series_id)
                .map_err(|error| format!("invalid NT option-chain series_id {series_id}: {error}"))
        })
        .collect()
}

fn wire_aggregate_greeks_custom_data_event_binding(
    profile_id: &str,
    source_id: &str,
    data_type: DataType,
    runtime: &IvRuntimeEngine,
) -> BoltV3IvCustomDataRuntimeEventBinding {
    let pattern = switchboard::get_custom_topic(&data_type).into();
    let runtime = runtime.clone();
    let profile_id = profile_id.to_string();
    let source_id = source_id.to_string();
    let handler = ShareableMessageHandler::from_typed(move |custom_data: &CustomData| {
        if let Err(error) = runtime.ingest_nt_aggregate_greeks_custom_data(
            &profile_id,
            &source_id,
            custom_data,
            iv_runtime_event_received_ts_ns(),
        ) {
            log::error!("bolt-v3 IV aggregate-greeks custom-data ingest failed: {error:?}");
        }
    });
    msgbus::subscribe_any(pattern, handler.clone(), None);
    BoltV3IvCustomDataRuntimeEventBinding { pattern, handler }
}

fn wire_custom_iv_event_binding(
    profile_id: &str,
    source_id: &str,
    data_type: DataType,
    runtime: &IvRuntimeEngine,
) -> BoltV3IvCustomDataRuntimeEventBinding {
    let pattern = switchboard::get_custom_topic(&data_type).into();
    let runtime = runtime.clone();
    let profile_id = profile_id.to_string();
    let source_id = source_id.to_string();
    let handler = ShareableMessageHandler::from_typed(move |custom_data: &CustomData| {
        if let Err(error) = runtime.ingest_nt_custom_iv_data(
            &profile_id,
            &source_id,
            custom_data,
            iv_runtime_event_received_ts_ns(),
        ) {
            log::error!("bolt-v3 IV custom-IV custom-data ingest failed: {error:?}");
        }
    });
    msgbus::subscribe_any(pattern, handler.clone(), None);
    BoltV3IvCustomDataRuntimeEventBinding { pattern, handler }
}

fn wire_option_greeks_event_binding(
    profile_id: &str,
    source_id: &str,
    instrument_id: InstrumentId,
    runtime: &IvRuntimeEngine,
) -> BoltV3IvOptionGreeksRuntimeEventBinding {
    let pattern = switchboard::get_option_greeks_topic(instrument_id).into();
    let runtime = runtime.clone();
    let profile_id = profile_id.to_string();
    let source_id = source_id.to_string();
    let handler = TypedHandler::from(move |option_greeks: &OptionGreeks| {
        if let Err(error) = runtime.ingest_nt_option_greeks(
            &profile_id,
            &source_id,
            option_greeks,
            iv_runtime_event_received_ts_ns(),
        ) {
            log::error!("bolt-v3 IV option-greeks event ingest failed: {error:?}");
        }
    });
    msgbus::subscribe_option_greeks(pattern, handler.clone(), None);
    BoltV3IvOptionGreeksRuntimeEventBinding { pattern, handler }
}

fn wire_option_chain_event_binding(
    profile_id: &str,
    source_id: &str,
    series_id: OptionSeriesId,
    runtime: &IvRuntimeEngine,
) -> BoltV3IvOptionChainRuntimeEventBinding {
    let pattern = switchboard::get_option_chain_topic(series_id).into();
    let runtime = runtime.clone();
    let profile_id = profile_id.to_string();
    let source_id = source_id.to_string();
    let handler = TypedHandler::from(move |option_chain: &OptionChainSlice| {
        if let Err(error) = runtime.ingest_nt_option_chain_slice(
            &profile_id,
            &source_id,
            option_chain,
            iv_runtime_event_received_ts_ns(),
        ) {
            log::error!("bolt-v3 IV option-chain event ingest failed: {error:?}");
        }
    });
    msgbus::subscribe_option_chain(pattern, handler.clone(), None);
    BoltV3IvOptionChainRuntimeEventBinding { pattern, handler }
}

fn iv_runtime_event_received_ts_ns() -> UnixNanos {
    UnixNanos::new(get_atomic_clock_realtime().get_time_ns().as_u64())
}

fn iv_runtime_event_binding_error(
    profile_id: &str,
    source_id: &str,
    message: String,
) -> BoltV3StrategyRegistrationError {
    BoltV3StrategyRegistrationError::IvQueryHandleRegistration {
        message: format!(
            "bolt-v3 IV runtime event binding failed for profile {profile_id} source {source_id}: {message}"
        ),
    }
}

fn iv_runtime_data_commands_for_plan(
    plan: &IvSubscriptionPlan,
) -> Result<Vec<DataCommand>, IvRuntimeBindingError> {
    let ts_init = get_atomic_clock_realtime().get_time_ns();
    let client_id = Some(ClientId::from(plan.client_id.as_str()));

    match (plan.operation, &plan.selector) {
        (
            IvRuntimeOperation::SubscribeOptionGreeks | IvRuntimeOperation::UnsubscribeOptionGreeks,
            IvSelector::SourceOptionGreeks {
                instrument_ids,
                nt_params,
            },
        ) => {
            let params = merged_nt_params(plan, nt_params)?;
            let commands = parse_option_greeks_instrument_ids(instrument_ids)
                .map_err(|message| binding_error(plan, message))?
                .into_iter()
                .map(|instrument_id| {
                    if plan.operation == IvRuntimeOperation::SubscribeOptionGreeks {
                        DataCommand::Subscribe(SubscribeCommand::OptionGreeks(
                            SubscribeOptionGreeks::new(
                                instrument_id,
                                client_id,
                                None,
                                UUID4::new(),
                                ts_init,
                                None,
                                params.clone(),
                            ),
                        ))
                    } else {
                        DataCommand::Unsubscribe(UnsubscribeCommand::OptionGreeks(
                            UnsubscribeOptionGreeks::new(
                                instrument_id,
                                client_id,
                                None,
                                UUID4::new(),
                                ts_init,
                                None,
                                params.clone(),
                            ),
                        ))
                    }
                })
                .collect();
            Ok(commands)
        }
        (
            IvRuntimeOperation::SubscribeOptionChain | IvRuntimeOperation::UnsubscribeOptionChain,
            IvSelector::SourceOptionChain {
                series_ids,
                strike_range_policy,
                nt_params,
            },
        ) => {
            let params = merged_nt_params(plan, nt_params)?;
            let strike_range = parse_nt_strike_range(plan, strike_range_policy)?;
            let snapshot_interval_ms = params
                .as_ref()
                .and_then(|params| params.get_u64("snapshot_interval_ms"));
            let commands = parse_option_chain_series_ids(series_ids)
                .map_err(|message| binding_error(plan, message))?
                .into_iter()
                .map(|series_id| {
                    if plan.operation == IvRuntimeOperation::SubscribeOptionChain {
                        DataCommand::Subscribe(SubscribeCommand::OptionChain(
                            SubscribeOptionChain::new(
                                series_id,
                                strike_range.clone(),
                                snapshot_interval_ms,
                                UUID4::new(),
                                ts_init,
                                client_id,
                                None,
                                params.clone(),
                            ),
                        ))
                    } else {
                        DataCommand::Unsubscribe(UnsubscribeCommand::OptionChain(
                            UnsubscribeOptionChain::new(
                                series_id,
                                UUID4::new(),
                                ts_init,
                                client_id,
                                None,
                            ),
                        ))
                    }
                })
                .collect();
            Ok(commands)
        }
        (
            IvRuntimeOperation::SubscribeCustomData | IvRuntimeOperation::UnsubscribeCustomData,
            IvSelector::SourceCustomImpliedVolatility {
                custom_iv_data_type,
                nt_params,
                ..
            },
        ) => {
            let (data_type, params) = custom_iv_data_type_for_source(
                &plan.source_id,
                custom_iv_data_type,
                &plan.params,
                nt_params,
            )
            .map_err(|message| binding_error(plan, message))?;
            Ok(vec![custom_data_command(
                plan.operation,
                client_id,
                data_type,
                params,
                ts_init,
            )])
        }
        (
            IvRuntimeOperation::SubscribeAggregateGreeks
            | IvRuntimeOperation::UnsubscribeAggregateGreeks,
            IvSelector::SourceAggregateGreeks {
                aggregate_key,
                underlying_selectors,
                nt_params,
                ..
            },
        ) => {
            let (data_type, params) = aggregate_greeks_data_type_for_source(
                &plan.source_id,
                aggregate_key,
                underlying_selectors,
                &plan.params,
                nt_params,
            )
            .map_err(|message| binding_error(plan, message))?;
            Ok(vec![custom_data_command(
                plan.operation,
                client_id,
                data_type,
                params,
                ts_init,
            )])
        }
        (IvRuntimeOperation::RemoveSource, _) => Ok(Vec::new()),
        _ => Err(binding_error(
            plan,
            "IV subscription plan operation does not match selector kind".to_string(),
        )),
    }
}

fn custom_data_command(
    operation: IvRuntimeOperation,
    client_id: Option<ClientId>,
    data_type: DataType,
    params: Option<Params>,
    ts_init: nautilus_core::UnixNanos,
) -> DataCommand {
    match operation {
        IvRuntimeOperation::SubscribeCustomData | IvRuntimeOperation::SubscribeAggregateGreeks => {
            DataCommand::Subscribe(SubscribeCommand::Data(SubscribeCustomData::new(
                client_id,
                None,
                data_type,
                UUID4::new(),
                ts_init,
                None,
                params,
            )))
        }
        IvRuntimeOperation::UnsubscribeCustomData
        | IvRuntimeOperation::UnsubscribeAggregateGreeks => {
            DataCommand::Unsubscribe(UnsubscribeCommand::Data(UnsubscribeCustomData::new(
                client_id,
                None,
                data_type,
                UUID4::new(),
                ts_init,
                None,
                params,
            )))
        }
        _ => unreachable!("custom data command requires a custom-data IV runtime operation"),
    }
}

struct NtIvRuntimeCommandSenderAdapter {
    allowed_data_client_ids: BTreeSet<ClientId>,
    external_client_ids: BTreeSet<ClientId>,
}

impl NtIvRuntimeCommandSenderAdapter {
    fn new(registered_data_clients: &[ClientId], configured_external_clients: &[ClientId]) -> Self {
        let mut allowed_data_client_ids = registered_data_clients
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        allowed_data_client_ids.extend(configured_external_clients.iter().cloned());
        Self {
            allowed_data_client_ids,
            external_client_ids: configured_external_clients.iter().cloned().collect(),
        }
    }

    fn is_external_client(&self, plan: &IvSubscriptionPlan) -> bool {
        self.external_client_ids
            .contains(&ClientId::from(plan.client_id.as_str()))
    }

    fn validate_client_id(&self, plan: &IvSubscriptionPlan) -> Result<(), IvRuntimeBindingError> {
        let client_id = ClientId::from(plan.client_id.as_str());
        if self.allowed_data_client_ids.contains(&client_id) {
            Ok(())
        } else {
            Err(binding_error(
                plan,
                format!(
                    "IV source client_id {} is not registered as an NT data client or configured external data client",
                    plan.client_id
                ),
            ))
        }
    }
}

impl IvRuntimeBindingAdapter for NtIvRuntimeCommandSenderAdapter {
    fn apply_subscription_plan(
        &mut self,
        plan: &IvSubscriptionPlan,
    ) -> Result<(), IvRuntimeBindingError> {
        if self.is_external_client(plan) {
            return Ok(());
        }
        self.validate_client_id(plan)?;

        let sender = get_data_cmd_sender();
        for command in iv_runtime_data_commands_for_plan(plan)? {
            sender.execute(command);
        }
        Ok(())
    }
}

struct NtIvRuntimePlanValidationAdapter {
    allowed_data_client_ids: BTreeSet<ClientId>,
}

impl NtIvRuntimePlanValidationAdapter {
    fn new(node: &LiveNode, configured_external_clients: &[ClientId]) -> Self {
        let mut allowed_data_client_ids = node
            .kernel()
            .data_engine
            .borrow()
            .registered_clients()
            .into_iter()
            .collect::<BTreeSet<_>>();
        allowed_data_client_ids.extend(configured_external_clients.iter().cloned());
        Self {
            allowed_data_client_ids,
        }
    }

    fn validate_client_id(&self, plan: &IvSubscriptionPlan) -> Result<(), IvRuntimeBindingError> {
        let client_id = ClientId::from(plan.client_id.as_str());
        if self.allowed_data_client_ids.contains(&client_id) {
            Ok(())
        } else {
            Err(binding_error(
                plan,
                format!(
                    "IV source client_id {} is not registered as an NT data client or configured external data client",
                    plan.client_id
                ),
            ))
        }
    }
}

impl IvRuntimeBindingAdapter for NtIvRuntimePlanValidationAdapter {
    fn apply_subscription_plan(
        &mut self,
        plan: &IvSubscriptionPlan,
    ) -> Result<(), IvRuntimeBindingError> {
        self.validate_client_id(plan)?;
        iv_runtime_data_commands_for_plan(plan)?;
        Ok(())
    }
}

struct NtIvRuntimeBindingAdapter<'a> {
    node: &'a mut LiveNode,
    allowed_data_client_ids: BTreeSet<ClientId>,
    external_client_ids: BTreeSet<ClientId>,
}

impl<'a> NtIvRuntimeBindingAdapter<'a> {
    fn new(node: &'a mut LiveNode, configured_external_clients: &[ClientId]) -> Self {
        let mut allowed_data_client_ids = node
            .kernel()
            .data_engine
            .borrow()
            .registered_clients()
            .into_iter()
            .collect::<BTreeSet<_>>();
        allowed_data_client_ids.extend(configured_external_clients.iter().cloned());
        Self {
            node,
            allowed_data_client_ids,
            external_client_ids: configured_external_clients.iter().cloned().collect(),
        }
    }

    fn is_external_client(&self, plan: &IvSubscriptionPlan) -> bool {
        self.external_client_ids
            .contains(&ClientId::from(plan.client_id.as_str()))
    }

    fn client_id(&self, plan: &IvSubscriptionPlan) -> Result<ClientId, IvRuntimeBindingError> {
        let client_id = ClientId::from(plan.client_id.as_str());
        if self.allowed_data_client_ids.contains(&client_id) {
            Ok(client_id)
        } else {
            Err(binding_error(
                plan,
                format!(
                    "IV source client_id {} is not registered as an NT data client or configured external data client",
                    plan.client_id
                ),
            ))
        }
    }

    fn apply_option_greeks(
        &mut self,
        plan: &IvSubscriptionPlan,
        instrument_ids: &[String],
        nt_params: &toml::Value,
        subscribe: bool,
    ) -> Result<(), IvRuntimeBindingError> {
        let params = merged_nt_params(plan, nt_params)?;
        let client_id = Some(self.client_id(plan)?);
        let instrument_ids = parse_option_greeks_instrument_ids(instrument_ids)
            .map_err(|message| binding_error(plan, message))?;
        for instrument_id in instrument_ids {
            let ts_init = self.node.kernel().clock.borrow().timestamp_ns();
            if subscribe {
                let command = SubscribeOptionGreeks::new(
                    instrument_id,
                    client_id,
                    None,
                    UUID4::new(),
                    ts_init,
                    None,
                    params.clone(),
                );
                self.node
                    .kernel()
                    .data_engine
                    .borrow_mut()
                    .execute_subscribe(SubscribeCommand::OptionGreeks(command))
                    .map_err(|error| binding_error(plan, error.to_string()))?;
            } else {
                let command = UnsubscribeOptionGreeks::new(
                    instrument_id,
                    client_id,
                    None,
                    UUID4::new(),
                    ts_init,
                    None,
                    params.clone(),
                );
                self.node
                    .kernel()
                    .data_engine
                    .borrow_mut()
                    .execute_unsubscribe(&UnsubscribeCommand::OptionGreeks(command))
                    .map_err(|error| binding_error(plan, error.to_string()))?;
            }
        }
        Ok(())
    }

    fn apply_option_chain(
        &mut self,
        plan: &IvSubscriptionPlan,
        series_ids: &[String],
        strike_range_policy: &str,
        nt_params: &toml::Value,
        subscribe: bool,
    ) -> Result<(), IvRuntimeBindingError> {
        let params = merged_nt_params(plan, nt_params)?;
        let strike_range = parse_nt_strike_range(plan, strike_range_policy)?;
        let snapshot_interval_ms = params
            .as_ref()
            .and_then(|params| params.get_u64("snapshot_interval_ms"));
        let client_id = Some(self.client_id(plan)?);
        let series_ids = parse_option_chain_series_ids(series_ids)
            .map_err(|message| binding_error(plan, message))?;
        for series_id in series_ids {
            let ts_init = self.node.kernel().clock.borrow().timestamp_ns();
            if subscribe {
                let command = SubscribeOptionChain::new(
                    series_id,
                    strike_range.clone(),
                    snapshot_interval_ms,
                    UUID4::new(),
                    ts_init,
                    client_id,
                    None,
                    params.clone(),
                );
                self.node
                    .kernel()
                    .data_engine
                    .borrow_mut()
                    .execute_subscribe(SubscribeCommand::OptionChain(command))
                    .map_err(|error| binding_error(plan, error.to_string()))?;
            } else {
                let command =
                    UnsubscribeOptionChain::new(series_id, UUID4::new(), ts_init, client_id, None);
                self.node
                    .kernel()
                    .data_engine
                    .borrow_mut()
                    .execute_unsubscribe(&UnsubscribeCommand::OptionChain(command))
                    .map_err(|error| binding_error(plan, error.to_string()))?;
            }
        }
        Ok(())
    }

    fn apply_custom_data(
        &mut self,
        plan: &IvSubscriptionPlan,
        custom_iv_data_type: &str,
        nt_params: &toml::Value,
        subscribe: bool,
    ) -> Result<(), IvRuntimeBindingError> {
        let (data_type, params) = custom_iv_data_type_for_source(
            &plan.source_id,
            custom_iv_data_type,
            &plan.params,
            nt_params,
        )
        .map_err(|message| binding_error(plan, message))?;
        self.execute_custom_data(plan, data_type, params, subscribe)
    }

    fn apply_aggregate_greeks(
        &mut self,
        plan: &IvSubscriptionPlan,
        aggregate_key: &str,
        underlying_selectors: &[String],
        nt_params: &toml::Value,
        subscribe: bool,
    ) -> Result<(), IvRuntimeBindingError> {
        let (data_type, params) = aggregate_greeks_data_type_for_source(
            &plan.source_id,
            aggregate_key,
            underlying_selectors,
            &plan.params,
            nt_params,
        )
        .map_err(|message| binding_error(plan, message))?;
        self.execute_custom_data(plan, data_type, params, subscribe)
    }

    fn execute_custom_data(
        &mut self,
        plan: &IvSubscriptionPlan,
        data_type: DataType,
        params: Option<Params>,
        subscribe: bool,
    ) -> Result<(), IvRuntimeBindingError> {
        let client_id = Some(self.client_id(plan)?);
        let ts_init = self.node.kernel().clock.borrow().timestamp_ns();
        if subscribe {
            let command = SubscribeCustomData::new(
                client_id,
                None,
                data_type,
                UUID4::new(),
                ts_init,
                None,
                params,
            );
            self.node
                .kernel()
                .data_engine
                .borrow_mut()
                .execute_subscribe(SubscribeCommand::Data(command))
                .map_err(|error| binding_error(plan, error.to_string()))?;
        } else {
            let command = UnsubscribeCustomData::new(
                client_id,
                None,
                data_type,
                UUID4::new(),
                ts_init,
                None,
                params,
            );
            self.node
                .kernel()
                .data_engine
                .borrow_mut()
                .execute_unsubscribe(&UnsubscribeCommand::Data(command))
                .map_err(|error| binding_error(plan, error.to_string()))?;
        }
        Ok(())
    }
}

impl IvRuntimeBindingAdapter for NtIvRuntimeBindingAdapter<'_> {
    fn apply_subscription_plan(
        &mut self,
        plan: &IvSubscriptionPlan,
    ) -> Result<(), IvRuntimeBindingError> {
        if self.is_external_client(plan) {
            return Ok(());
        }

        match (plan.operation, &plan.selector) {
            (
                IvRuntimeOperation::SubscribeOptionGreeks
                | IvRuntimeOperation::UnsubscribeOptionGreeks,
                IvSelector::SourceOptionGreeks {
                    instrument_ids,
                    nt_params,
                },
            ) => self.apply_option_greeks(
                plan,
                instrument_ids,
                nt_params,
                plan.operation == IvRuntimeOperation::SubscribeOptionGreeks,
            ),
            (
                IvRuntimeOperation::SubscribeOptionChain
                | IvRuntimeOperation::UnsubscribeOptionChain,
                IvSelector::SourceOptionChain {
                    series_ids,
                    strike_range_policy,
                    nt_params,
                },
            ) => self.apply_option_chain(
                plan,
                series_ids,
                strike_range_policy,
                nt_params,
                plan.operation == IvRuntimeOperation::SubscribeOptionChain,
            ),
            (
                IvRuntimeOperation::SubscribeCustomData | IvRuntimeOperation::UnsubscribeCustomData,
                IvSelector::SourceCustomImpliedVolatility {
                    custom_iv_data_type,
                    nt_params,
                    ..
                },
            ) => self.apply_custom_data(
                plan,
                custom_iv_data_type,
                nt_params,
                plan.operation == IvRuntimeOperation::SubscribeCustomData,
            ),
            (
                IvRuntimeOperation::SubscribeAggregateGreeks
                | IvRuntimeOperation::UnsubscribeAggregateGreeks,
                IvSelector::SourceAggregateGreeks {
                    aggregate_key,
                    underlying_selectors,
                    nt_params,
                    ..
                },
            ) => self.apply_aggregate_greeks(
                plan,
                aggregate_key,
                underlying_selectors,
                nt_params,
                plan.operation == IvRuntimeOperation::SubscribeAggregateGreeks,
            ),
            (IvRuntimeOperation::RemoveSource, _) => Ok(()),
            _ => Err(binding_error(
                plan,
                "IV subscription operation does not match selector kind".to_string(),
            )),
        }
    }
}

fn binding_error(plan: &IvSubscriptionPlan, message: String) -> IvRuntimeBindingError {
    IvRuntimeBindingError::subscription_failed(plan, message)
}

fn custom_iv_data_type_for_source(
    source_id: &str,
    custom_iv_data_type: &str,
    source_params: &toml::Value,
    selector_nt_params: &toml::Value,
) -> Result<(DataType, Option<Params>), String> {
    let params = merged_nt_params_from_values(source_params, selector_nt_params)?;
    let data_type = DataType::new(
        custom_iv_data_type,
        params.clone(),
        Some(source_id.to_string()),
    );
    Ok((data_type, params))
}

fn aggregate_greeks_data_type_for_source(
    source_id: &str,
    aggregate_key: &str,
    underlying_selectors: &[String],
    source_params: &toml::Value,
    selector_nt_params: &toml::Value,
) -> Result<(DataType, Option<Params>), String> {
    let mut params = merged_nt_params_from_values(source_params, selector_nt_params)?
        .unwrap_or_else(Params::new);
    params.insert(
        "underlying_selectors".to_string(),
        serde_json::Value::Array(
            underlying_selectors
                .iter()
                .cloned()
                .map(serde_json::Value::String)
                .collect(),
        ),
    );
    let params = Some(params);
    let data_type = DataType::new(aggregate_key, params.clone(), Some(source_id.to_string()));
    Ok((data_type, params))
}

fn merged_nt_params(
    plan: &IvSubscriptionPlan,
    selector_nt_params: &toml::Value,
) -> Result<Option<Params>, IvRuntimeBindingError> {
    merged_nt_params_from_values(&plan.params, selector_nt_params)
        .map_err(|message| binding_error(plan, message))
}

fn merged_nt_params_from_values(
    source_params: &toml::Value,
    selector_nt_params: &toml::Value,
) -> Result<Option<Params>, String> {
    let mut params = Params::new();
    insert_toml_params(&mut params, source_params, "source params")?;
    insert_toml_params(&mut params, selector_nt_params, "selector nt_params")?;
    if params.is_empty() {
        Ok(None)
    } else {
        Ok(Some(params))
    }
}

fn insert_toml_params(params: &mut Params, value: &toml::Value, label: &str) -> Result<(), String> {
    let toml::Value::Table(table) = value else {
        return Err(format!(
            "{label} must be a TOML table for NT params conversion"
        ));
    };
    for (key, value) in table {
        let value = serde_json::to_value(value).map_err(|error| {
            format!("failed to convert {label} key {key} into NT params: {error}")
        })?;
        params.insert(key.clone(), value);
    }
    Ok(())
}

fn parse_nt_strike_range(
    plan: &IvSubscriptionPlan,
    strike_range_policy: &str,
) -> Result<StrikeRange, IvRuntimeBindingError> {
    if let Some(pct) = strike_range_policy.strip_prefix("atm_percent:") {
        return pct
            .parse::<f64>()
            .map(|pct| StrikeRange::AtmPercent { pct })
            .map_err(|error| {
                binding_error(
                    plan,
                    format!("invalid atm_percent strike range policy: {error}"),
                )
            });
    }
    if let Some(relative) = strike_range_policy.strip_prefix("atm_relative:") {
        let Some((above, below)) = relative.split_once(':') else {
            return Err(binding_error(
                plan,
                "atm_relative strike range policy must be atm_relative:<above>:<below>".to_string(),
            ));
        };
        let strikes_above = above.parse::<usize>().map_err(|error| {
            binding_error(
                plan,
                format!("invalid atm_relative strikes_above value: {error}"),
            )
        })?;
        let strikes_below = below.parse::<usize>().map_err(|error| {
            binding_error(
                plan,
                format!("invalid atm_relative strikes_below value: {error}"),
            )
        })?;
        return Ok(StrikeRange::AtmRelative {
            strikes_above,
            strikes_below,
        });
    }
    if let Some(fixed) = strike_range_policy.strip_prefix("fixed:") {
        let mut strikes = Vec::new();
        for strike in fixed.split(',') {
            strikes.push(Price::from_str(strike.trim()).map_err(|error| {
                binding_error(plan, format!("invalid fixed strike range value: {error}"))
            })?);
        }
        return Ok(StrikeRange::Fixed(strikes));
    }
    Err(binding_error(
        plan,
        "strike_range_policy must be parseable as atm_percent:<pct>, atm_relative:<above>:<below>, or fixed:<strike,...>".to_string(),
    ))
}

mod strategy_free_probe {
    use super::*;

    #[cfg_attr(not(test), allow(dead_code))]
    #[derive(Debug, Clone, PartialEq)]
    pub struct BoltV3StrategyFreeReferenceQuote {
        pub data_client_id: String,
        pub instrument_id: String,
        pub bid_price: f64,
        pub ask_price: f64,
        pub ts_event_unix_nanos: u64,
        pub ts_init_unix_nanos: u64,
        pub captured_at_unix_nanos: u64,
    }

    #[cfg(test)]
    #[derive(Debug, Clone, PartialEq)]
    pub struct BoltV3StrategyFreeReferenceQuoteEvidence {
        pub quotes: Vec<BoltV3StrategyFreeReferenceQuote>,
    }

    #[cfg_attr(not(test), allow(dead_code))]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct BoltV3StrategyFreeBookDeltas {
        pub data_client_id: String,
        pub instrument_id: String,
        pub delta_count: u64,
        pub ts_event_unix_nanos: u64,
        pub ts_init_unix_nanos: u64,
        pub captured_at_unix_nanos: u64,
    }

    #[cfg(test)]
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct BoltV3StrategyFreeBookDeltasEvidence {
        pub deltas: Vec<BoltV3StrategyFreeBookDeltas>,
    }

    #[cfg_attr(not(test), allow(dead_code))]
    #[derive(Debug, Clone, PartialEq)]
    pub struct BoltV3StrategyFreeTrade {
        pub data_client_id: String,
        pub instrument_id: String,
        pub price: f64,
        pub size: f64,
        pub ts_event_unix_nanos: u64,
        pub ts_init_unix_nanos: u64,
        pub captured_at_unix_nanos: u64,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct StrategyFreeReferenceQuoteSubscription {
        pub(super) data_client_id: ClientId,
        pub(super) instrument_id: InstrumentId,
    }

    /// Live state for a trade chunk-count readiness walk. The probe subscribes one
    /// chunk of the instrument universe at a time (so it never holds more than
    /// `chunk_size` channels at once, staying below the venue's silent delivery
    /// ceiling), watches it for `chunk_observation_window_seconds`, then advances.
    /// It passes as soon as `required_live_markets` (`m`) distinct markets have
    /// traded, and fails closed once the whole universe has been walked without
    /// reaching `m`. Interior mutability mirrors the surrounding handle: the actor
    /// is single-threaded (`!Send`), so `Cell`/`RefCell` is sufficient.
    #[cfg_attr(not(test), allow(dead_code))]
    #[derive(Debug)]
    struct ChunkCountWalk {
        data_client_id: ClientId,
        chunk_size: usize,
        chunk_observation_window_seconds: u64,
        required_live_markets: usize,
        /// Universe pre-split into consecutive chunks of at most `chunk_size`,
        /// populated when the metadata response arrives.
        chunks: RefCell<Vec<Vec<InstrumentId>>>,
        /// Index of the next chunk to subscribe.
        cursor: Cell<usize>,
        /// Set once the universe has been captured and chunking has begun.
        started: Cell<bool>,
        /// Set once the walk has finished, whether by reaching `m` (pass) or by
        /// exhausting the universe (fail closed).
        complete: Cell<bool>,
        /// Distinct markets that fired at least one trade across subscribed
        /// chunks. Instrument IDs are enough here because the chunk-count probe is
        /// scoped to one data client.
        fired_instrument_ids: RefCell<BTreeSet<String>>,
    }

    #[derive(Debug, Clone)]
    pub(super) struct BoltV3StrategyFreeReferenceQuoteProbeHandle {
        pub(super) required: Rc<RefCell<Vec<StrategyFreeReferenceQuoteSubscription>>>,
        pub(super) ambiguous_instrument_ids: Rc<RefCell<BTreeSet<String>>>,
        pub(super) market_data_kind: DataClientReadinessProbeMarketDataKind,
        pub(super) metadata_response_data_client_id: Option<ClientId>,
        pub(super) metadata_response_max_quote_targets: Option<usize>,
        pub(super) metadata_response_allow_target_sampling: bool,
        pub(super) min_observed_targets: Option<usize>,
        pub(super) quote_targets_initialized: Rc<Cell<bool>>,
        pub(super) failure_reason: Rc<RefCell<Option<String>>>,
        pub(super) quotes: Rc<RefCell<Vec<BoltV3StrategyFreeReferenceQuote>>>,
        pub(super) book_deltas: Rc<RefCell<Vec<BoltV3StrategyFreeBookDeltas>>>,
        pub(super) trades: Rc<RefCell<Vec<BoltV3StrategyFreeTrade>>>,
        pub(super) quote_notify: Rc<tokio::sync::Notify>,
        /// Present only for a trade chunk-count probe (`market_data_kind = "trade"`
        /// with `quote_target_source = "metadata_response"`); drives the chunked
        /// walk over the instrument universe instead of a fixed sampled target set.
        chunk_walk: Option<Rc<ChunkCountWalk>>,
    }

    impl BoltV3StrategyFreeReferenceQuoteProbeHandle {
        pub(super) fn from_plan(
            required: Vec<StrategyFreeReferenceQuoteSubscription>,
            ambiguous_instrument_ids: BTreeSet<String>,
            market_data_kind: DataClientReadinessProbeMarketDataKind,
            min_observed_targets: Option<usize>,
        ) -> Self {
            Self {
                required: Rc::new(RefCell::new(required)),
                ambiguous_instrument_ids: Rc::new(RefCell::new(ambiguous_instrument_ids)),
                market_data_kind,
                metadata_response_data_client_id: None,
                metadata_response_max_quote_targets: None,
                metadata_response_allow_target_sampling: false,
                min_observed_targets,
                quote_targets_initialized: Rc::new(Cell::new(true)),
                failure_reason: Rc::new(RefCell::new(None)),
                quotes: Rc::new(RefCell::new(Vec::new())),
                book_deltas: Rc::new(RefCell::new(Vec::new())),
                trades: Rc::new(RefCell::new(Vec::new())),
                quote_notify: Rc::new(tokio::sync::Notify::new()),
                chunk_walk: None,
            }
        }

        pub(super) fn from_metadata_response_plan(
            data_client_id: ClientId,
            max_quote_targets: usize,
            allow_target_sampling: bool,
            market_data_kind: DataClientReadinessProbeMarketDataKind,
            min_observed_targets: Option<usize>,
        ) -> Self {
            Self {
                required: Rc::new(RefCell::new(Vec::new())),
                ambiguous_instrument_ids: Rc::new(RefCell::new(BTreeSet::new())),
                market_data_kind,
                metadata_response_data_client_id: Some(data_client_id),
                metadata_response_max_quote_targets: Some(max_quote_targets),
                metadata_response_allow_target_sampling: allow_target_sampling,
                min_observed_targets,
                quote_targets_initialized: Rc::new(Cell::new(false)),
                failure_reason: Rc::new(RefCell::new(None)),
                quotes: Rc::new(RefCell::new(Vec::new())),
                book_deltas: Rc::new(RefCell::new(Vec::new())),
                trades: Rc::new(RefCell::new(Vec::new())),
                quote_notify: Rc::new(tokio::sync::Notify::new()),
                chunk_walk: None,
            }
        }

        pub(super) fn from_metadata_response_chunk_count_plan(
            data_client_id: ClientId,
            chunk_size: usize,
            chunk_observation_window_seconds: u64,
            required_live_markets: usize,
            market_data_kind: DataClientReadinessProbeMarketDataKind,
        ) -> Self {
            Self {
                required: Rc::new(RefCell::new(Vec::new())),
                ambiguous_instrument_ids: Rc::new(RefCell::new(BTreeSet::new())),
                market_data_kind,
                metadata_response_data_client_id: Some(data_client_id),
                metadata_response_max_quote_targets: None,
                metadata_response_allow_target_sampling: false,
                min_observed_targets: Some(required_live_markets),
                quote_targets_initialized: Rc::new(Cell::new(false)),
                failure_reason: Rc::new(RefCell::new(None)),
                quotes: Rc::new(RefCell::new(Vec::new())),
                book_deltas: Rc::new(RefCell::new(Vec::new())),
                trades: Rc::new(RefCell::new(Vec::new())),
                quote_notify: Rc::new(tokio::sync::Notify::new()),
                chunk_walk: Some(Rc::new(ChunkCountWalk {
                    data_client_id,
                    chunk_size,
                    chunk_observation_window_seconds,
                    required_live_markets,
                    chunks: RefCell::new(Vec::new()),
                    cursor: Cell::new(0),
                    started: Cell::new(false),
                    complete: Cell::new(false),
                    fired_instrument_ids: RefCell::new(BTreeSet::new()),
                })),
            }
        }

        pub(super) fn is_chunk_count_mode(&self) -> bool {
            self.chunk_walk.is_some()
        }

        /// Capture the metadata-response universe and split it into chunks. The
        /// universe is sorted and de-duplicated so chunk membership is
        /// deterministic; which markets ultimately certify the feed is still
        /// liveness-driven (a chunk's markets only count once they actually trade).
        #[cfg(test)]
        pub(super) fn chunk_count_capture_universe(&self, mut instrument_ids: Vec<InstrumentId>) {
            let Some(walk) = &self.chunk_walk else {
                return;
            };
            if walk.started.get() {
                return;
            }
            instrument_ids.sort_by_key(|instrument_id| instrument_id.to_string());
            instrument_ids.dedup();
            *walk.chunks.borrow_mut() = chunk_universe(&instrument_ids, walk.chunk_size);
            walk.cursor.set(0);
            walk.complete.set(false);
            walk.fired_instrument_ids.borrow_mut().clear();
            walk.started.set(true);
            self.quote_notify.notify_one();
        }

        /// Take the next chunk to subscribe, installing it as the probe's current
        /// `required` set so recorded trades match against it. Returns `None` once
        /// the universe is exhausted.
        #[cfg(test)]
        pub(super) fn chunk_count_next_chunk(
            &self,
        ) -> Option<Vec<StrategyFreeReferenceQuoteSubscription>> {
            let walk = self.chunk_walk.as_ref()?;
            let cursor = walk.cursor.get();
            let chunk = match walk.chunks.borrow().get(cursor).cloned() {
                Some(chunk) => chunk,
                None => {
                    walk.complete.set(true);
                    if !self.chunk_count_passed() {
                        self.fail_metadata_response_probe(format!(
                            "trade chunk-count readiness probe exhausted {} chunk(s) with {} distinct fired market(s), below required min_observed_targets={}",
                            walk.chunks.borrow().len(),
                            walk.fired_instrument_ids.borrow().len(),
                            walk.required_live_markets,
                        ));
                    }
                    return None;
                }
            };
            walk.cursor.set(cursor + 1);
            let subscriptions: Vec<StrategyFreeReferenceQuoteSubscription> = chunk
                .into_iter()
                .map(|instrument_id| StrategyFreeReferenceQuoteSubscription {
                    data_client_id: walk.data_client_id,
                    instrument_id,
                })
                .collect();
            *self.required.borrow_mut() = subscriptions.clone();
            Some(subscriptions)
        }

        /// The chunk currently subscribed, returned so the actor can unsubscribe it
        /// before advancing to the next chunk.
        #[cfg(test)]
        pub(super) fn chunk_count_current_chunk(
            &self,
        ) -> Vec<StrategyFreeReferenceQuoteSubscription> {
            self.required.borrow().clone()
        }

        pub(super) fn chunk_count_passed(&self) -> bool {
            match &self.chunk_walk {
                Some(walk) => trade_chunk_count_probe_passed(
                    walk.fired_instrument_ids.borrow().len(),
                    walk.required_live_markets,
                ),
                None => false,
            }
        }

        #[cfg(test)]
        pub(super) fn chunk_walk_started(&self) -> bool {
            self.chunk_walk
                .as_ref()
                .is_some_and(|walk| walk.started.get())
        }

        /// `(number_of_chunks, per_chunk_window_seconds)` for sizing the overall
        /// walk timeout once the universe is known.
        #[cfg(test)]
        pub(super) fn chunk_walk_dims(&self) -> (usize, u64) {
            match &self.chunk_walk {
                Some(walk) => (
                    walk.chunks.borrow().len(),
                    walk.chunk_observation_window_seconds,
                ),
                None => (0, 0),
            }
        }

        #[cfg(test)]
        pub(super) fn has_all_required_quotes(&self) -> bool {
            if self.market_data_kind != DataClientReadinessProbeMarketDataKind::Quote {
                return false;
            }
            self.has_all_required_market_data()
        }

        pub(super) fn has_all_required_market_data(&self) -> bool {
            if self.failure_error().is_some() {
                return false;
            }
            if let Some(walk) = &self.chunk_walk {
                // Chunk-count probe: satisfied once the walk has concluded by
                // reaching `m` distinct firing markets. Fail-closed exhaustion sets
                // `failure_error` (handled above), so reaching here with
                // `complete` set and the pass rule unmet cannot happen.
                return walk.complete.get() && self.chunk_count_passed();
            }
            if !self.ambiguous_instrument_ids.borrow().is_empty() {
                return false;
            }
            if !self.quote_targets_initialized.get() {
                return false;
            }
            let required = self.required.borrow();
            if self.metadata_response_data_client_id.is_some() && required.is_empty() {
                return false;
            }
            let required_observations = self.required_observation_count(required.len());
            match self.market_data_kind {
                DataClientReadinessProbeMarketDataKind::Quote => {
                    let quotes = self.quotes.borrow();
                    observed_required_quote_count(&required, &quotes) >= required_observations
                }
                DataClientReadinessProbeMarketDataKind::Book => {
                    let book_deltas = self.book_deltas.borrow();
                    observed_required_book_delta_count(&required, &book_deltas)
                        >= required_observations
                }
                DataClientReadinessProbeMarketDataKind::Trade => {
                    let trades = self.trades.borrow();
                    observed_required_trade_count(&required, &trades) >= required_observations
                }
            }
        }

        pub(super) fn required_market_data_count(&self) -> usize {
            if let Some(walk) = &self.chunk_walk {
                return walk.required_live_markets;
            }
            let required_len = self.required.borrow().len();
            if self.metadata_response_data_client_id.is_some() && required_len == 0 {
                return self
                    .min_observed_targets
                    .or(self.metadata_response_max_quote_targets)
                    .unwrap_or(0);
            }
            self.required_observation_count(required_len)
        }

        pub(super) fn observed_market_data_count(&self) -> usize {
            if let Some(walk) = &self.chunk_walk {
                return walk.fired_instrument_ids.borrow().len();
            }
            let required = self.required.borrow();
            match self.market_data_kind {
                DataClientReadinessProbeMarketDataKind::Quote => {
                    observed_required_quote_count(&required, &self.quotes.borrow())
                }
                DataClientReadinessProbeMarketDataKind::Book => {
                    observed_required_book_delta_count(&required, &self.book_deltas.borrow())
                }
                DataClientReadinessProbeMarketDataKind::Trade => {
                    observed_required_trade_count(&required, &self.trades.borrow())
                }
            }
        }

        /// Number of sampled targets that must be observed for the probe to pass.
        ///
        /// Defaults to every sampled target (strict, fail-closed). When
        /// `readiness_probe.min_observed_targets` is configured it lowers the bar to
        /// that value, clamped into `[1, sampled_len]` so a broad metadata universe
        /// can prove adapter data-path behaviour without requiring every illiquid or
        /// un-streamable sampled instrument to tick within the configured wait.
        fn required_observation_count(&self, sampled_len: usize) -> usize {
            match self.min_observed_targets {
                Some(min_observed) => min_observed.clamp(1, sampled_len.max(1)),
                None => sampled_len,
            }
        }

        pub(super) fn failure_error(&self) -> Option<String> {
            self.failure_reason.borrow().clone()
        }

        fn fail_metadata_response_probe(&self, reason: String) {
            if self.failure_reason.borrow().is_none() {
                *self.failure_reason.borrow_mut() = Some(reason);
            }
            self.required.borrow_mut().clear();
            self.ambiguous_instrument_ids.borrow_mut().clear();
            self.quote_targets_initialized.set(true);
            self.quote_notify.notify_one();
        }

        #[cfg(test)]
        pub(super) fn evidence(&self) -> BoltV3StrategyFreeReferenceQuoteEvidence {
            BoltV3StrategyFreeReferenceQuoteEvidence {
                quotes: self.quotes.borrow().clone(),
            }
        }

        #[cfg(test)]
        pub(super) fn book_evidence(&self) -> BoltV3StrategyFreeBookDeltasEvidence {
            BoltV3StrategyFreeBookDeltasEvidence {
                deltas: self.book_deltas.borrow().clone(),
            }
        }

        pub(super) fn install_metadata_response_instrument_ids(
            &self,
            mut instrument_ids: Vec<InstrumentId>,
        ) -> Vec<StrategyFreeReferenceQuoteSubscription> {
            let Some(data_client_id) = self.metadata_response_data_client_id else {
                return Vec::new();
            };
            if self.quote_targets_initialized.get() {
                return Vec::new();
            }
            instrument_ids.sort_by_key(|instrument_id| instrument_id.to_string());
            instrument_ids.dedup();
            let Some(max_quote_targets) = self.metadata_response_max_quote_targets else {
                self.fail_metadata_response_probe(
                "clients.<id>.readiness_probe.max_metadata_quote_targets is missing for metadata_response readiness probing".to_string(),
            );
                return Vec::new();
            };
            let metadata_quote_targets = instrument_ids.len();
            if metadata_quote_targets > max_quote_targets {
                if self.metadata_response_allow_target_sampling {
                    instrument_ids =
                        sample_metadata_response_targets(&instrument_ids, max_quote_targets);
                } else {
                    self.fail_metadata_response_probe(format!(
                    "metadata_response produced {metadata_quote_targets} source-owned quote targets, exceeding clients.<id>.readiness_probe.max_metadata_quote_targets={max_quote_targets}; tighten TOML-owned metadata filters or set clients.<id>.readiness_probe.allow_metadata_target_sampling=true before using this client for production readiness"
                ));
                    return Vec::new();
                }
            }
            let subscriptions = instrument_ids
                .into_iter()
                .map(|instrument_id| StrategyFreeReferenceQuoteSubscription {
                    data_client_id,
                    instrument_id,
                })
                .collect();
            let (required, ambiguous_instrument_ids) =
                dedupe_strategy_free_reference_quote_subscriptions(subscriptions);
            if let Some(min_observed) = self.min_observed_targets
                && min_observed > required.len()
            {
                self.fail_metadata_response_probe(format!(
                "clients.<id>.readiness_probe.min_observed_targets={min_observed} exceeds the {} source-owned metadata_response target(s) sampled this run",
                required.len()
            ));
                return Vec::new();
            }
            *self.required.borrow_mut() = required.clone();
            *self.ambiguous_instrument_ids.borrow_mut() = ambiguous_instrument_ids;
            self.quote_targets_initialized.set(true);
            self.quote_notify.notify_one();
            required
        }

        pub(super) fn record_quote(&self, quote: &QuoteTick, captured_at_unix_nanos: u64) {
            let quote_instrument_id = quote.instrument_id.to_string();
            if self
                .ambiguous_instrument_ids
                .borrow()
                .contains(&quote_instrument_id)
            {
                return;
            }
            let required = self.required.borrow().clone();
            let mut matched_required = false;
            let mut quotes = self.quotes.borrow_mut();
            for required in &required {
                if quote.instrument_id == required.instrument_id {
                    matched_required = true;
                    quotes.push(BoltV3StrategyFreeReferenceQuote {
                        data_client_id: required.data_client_id.to_string(),
                        instrument_id: required.instrument_id.to_string(),
                        bid_price: quote.bid_price.as_f64(),
                        ask_price: quote.ask_price.as_f64(),
                        ts_event_unix_nanos: quote.ts_event.as_u64(),
                        ts_init_unix_nanos: quote.ts_init.as_u64(),
                        captured_at_unix_nanos,
                    });
                }
            }
            drop(quotes);
            if matched_required && self.has_all_required_market_data() {
                self.quote_notify.notify_one();
            }
        }

        pub(super) fn record_book_deltas(
            &self,
            deltas: &OrderBookDeltas,
            captured_at_unix_nanos: u64,
        ) {
            let deltas_instrument_id = deltas.instrument_id.to_string();
            if self
                .ambiguous_instrument_ids
                .borrow()
                .contains(&deltas_instrument_id)
            {
                return;
            }
            let required = self.required.borrow().clone();
            let mut matched_required = false;
            let mut book_deltas = self.book_deltas.borrow_mut();
            for required in &required {
                if deltas.instrument_id == required.instrument_id {
                    matched_required = true;
                    book_deltas.push(BoltV3StrategyFreeBookDeltas {
                        data_client_id: required.data_client_id.to_string(),
                        instrument_id: required.instrument_id.to_string(),
                        delta_count: deltas.deltas.len() as u64,
                        ts_event_unix_nanos: deltas.ts_event.as_u64(),
                        ts_init_unix_nanos: deltas.ts_init.as_u64(),
                        captured_at_unix_nanos,
                    });
                }
            }
            drop(book_deltas);
            if matched_required && self.has_all_required_market_data() {
                self.quote_notify.notify_one();
            }
        }

        pub(super) fn record_trade(&self, trade: &TradeTick) {
            if self.market_data_kind != DataClientReadinessProbeMarketDataKind::Trade {
                return;
            }
            if let Some(walk) = &self.chunk_walk {
                if walk.complete.get() {
                    return;
                }
                if self
                    .required
                    .borrow()
                    .iter()
                    .any(|required| trade.instrument_id == required.instrument_id)
                {
                    walk.fired_instrument_ids
                        .borrow_mut()
                        .insert(trade.instrument_id.to_string());
                    if self.chunk_count_passed() {
                        walk.complete.set(true);
                        self.quote_notify.notify_one();
                    }
                }
                return;
            }
            let trade_instrument_id = trade.instrument_id.to_string();
            if self
                .ambiguous_instrument_ids
                .borrow()
                .contains(&trade_instrument_id)
            {
                return;
            }
            let mut matched_required = false;
            {
                let required = self.required.borrow();
                let mut trades = self.trades.borrow_mut();
                for required in required.iter() {
                    if trade.instrument_id == required.instrument_id {
                        matched_required = true;
                        trades.push(BoltV3StrategyFreeTrade {
                            data_client_id: required.data_client_id.to_string(),
                            instrument_id: required.instrument_id.to_string(),
                            price: trade.price.as_f64(),
                            size: trade.size.as_f64(),
                            ts_event_unix_nanos: trade.ts_event.as_u64(),
                            ts_init_unix_nanos: trade.ts_init.as_u64(),
                            captured_at_unix_nanos: get_atomic_clock_realtime()
                                .get_time_ns()
                                .as_u64(),
                        });
                    }
                }
            }
            if matched_required && self.has_all_required_market_data() {
                self.quote_notify.notify_one();
            }
        }

        #[cfg(test)]
        pub(super) async fn wait_for_all_required_quotes(&self) -> Result<(), String> {
            loop {
                if let Some(reason) = self.failure_error() {
                    return Err(reason);
                }
                if self.has_all_required_market_data() {
                    return Ok(());
                }
                self.quote_notify.notified().await;
            }
        }
    }

    pub(crate) fn sample_metadata_response_targets<T: Clone>(
        targets: &[T],
        max_targets: usize,
    ) -> Vec<T> {
        if max_targets == 0 {
            return Vec::new();
        }
        if targets.len() <= max_targets {
            return targets.to_vec();
        }
        if max_targets == 1 {
            return vec![targets[targets.len() / 2].clone()];
        }
        let last_index = targets.len() - 1;
        let last_sample = max_targets - 1;
        (0..max_targets)
            .map(|sample_index| targets[(sample_index * last_index) / last_sample].clone())
            .collect()
    }

    /// Split a deterministically-ordered instrument universe into consecutive
    /// chunks of at most `chunk_size`, preserving order. Returns no chunks when
    /// `chunk_size == 0` so a misconfigured probe observes nothing and fails
    /// closed rather than panicking. The trade chunk-count readiness probe walks
    /// the universe one chunk at a time so it never subscribes to more than
    /// `chunk_size` channels at once, staying below the venue's silent delivery
    /// ceiling.
    #[cfg(test)]
    pub(crate) fn chunk_universe<T: Clone>(universe: &[T], chunk_size: usize) -> Vec<Vec<T>> {
        if chunk_size == 0 {
            return Vec::new();
        }
        universe.chunks(chunk_size).map(<[T]>::to_vec).collect()
    }

    /// Pass rule for a trade chunk-count readiness probe: at least
    /// `required_live_markets` (`m`) distinct markets must have produced a trade
    /// across the chunk walk, and `m` itself must be >= 1 (a probe that requires
    /// nothing proves nothing, so it fails closed). Single source of truth for
    /// the pass decision, shared by the live probe orchestration and the
    /// operator-artifacts materializer so both agree on what "healthy" means.
    pub(crate) fn trade_chunk_count_probe_passed(
        distinct_fired: usize,
        required_live_markets: usize,
    ) -> bool {
        required_live_markets >= 1 && distinct_fired >= required_live_markets
    }

    fn observed_required_book_delta_count(
        required: &[StrategyFreeReferenceQuoteSubscription],
        book_deltas: &[BoltV3StrategyFreeBookDeltas],
    ) -> usize {
        let mut observed = BTreeSet::new();
        for required in required {
            if book_deltas.iter().any(|deltas| {
                deltas.data_client_id == required.data_client_id.to_string()
                    && deltas.instrument_id == required.instrument_id.to_string()
            }) {
                observed.insert((
                    required.data_client_id.to_string(),
                    required.instrument_id.to_string(),
                ));
            }
        }
        observed.len()
    }

    fn observed_required_quote_count(
        required: &[StrategyFreeReferenceQuoteSubscription],
        quotes: &[BoltV3StrategyFreeReferenceQuote],
    ) -> usize {
        let mut observed = BTreeSet::new();
        for required in required {
            if quotes.iter().any(|quote| {
                quote.data_client_id == required.data_client_id.to_string()
                    && quote.instrument_id == required.instrument_id.to_string()
            }) {
                observed.insert((
                    required.data_client_id.to_string(),
                    required.instrument_id.to_string(),
                ));
            }
        }
        observed.len()
    }

    fn observed_required_trade_count(
        required: &[StrategyFreeReferenceQuoteSubscription],
        trades: &[BoltV3StrategyFreeTrade],
    ) -> usize {
        let mut observed = BTreeSet::new();
        for required in required {
            let required_instrument_id = required.instrument_id.to_string();
            if trades.iter().any(|trade| {
                trade.data_client_id.as_str() == required.data_client_id.as_str()
                    && trade.instrument_id.as_str() == required_instrument_id.as_str()
            }) {
                observed.insert((&required.data_client_id, &required.instrument_id));
            }
        }
        observed.len()
    }

    pub(super) fn strategy_free_data_client_readiness_quote_subscription_plan(
        loaded: &LoadedBoltV3Config,
        client_key: &str,
    ) -> Result<
        (
            Vec<StrategyFreeReferenceQuoteSubscription>,
            BTreeSet<String>,
        ),
        BoltV3LiveNodeError,
    > {
        let client = loaded.root.clients.get(client_key).ok_or_else(|| {
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                "data-client readiness quote probe client_key is not configured"
            ))
        })?;
        if client.data.is_none() {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "data-client readiness quote probe requires the selected client to declare [data]"
                ),
            ));
        }
        let readiness_probe = client.readiness_probe.as_ref().ok_or_else(|| {
        BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
            "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
        ))
    })?;
        if readiness_probe.quote_target_source
            != DataClientReadinessProbeQuoteTargetSource::Configured
        {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "standalone data-client readiness quote probe requires quote_target_source = \"configured\"; metadata_response requires the combined data-client readiness probe"
                ),
            ));
        }
        let Some(quote_targets) = &readiness_probe.quote_targets else {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
                ),
            ));
        };
        if quote_targets.is_empty() {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
                ),
            ));
        }
        let subscriptions = quote_targets
            .values()
            .map(|target| StrategyFreeReferenceQuoteSubscription {
                data_client_id: ClientId::from(client_key),
                instrument_id: target.instrument_id,
            })
            .collect();
        Ok(dedupe_strategy_free_reference_quote_subscriptions(
            subscriptions,
        ))
    }

    /// Validates the TOML-owned `readiness_probe.min_observed_targets` lower bound.
    ///
    /// `min_observed_targets` lets a broad metadata universe prove adapter data-path
    /// behaviour by observing fresh data for at least this many sampled targets,
    /// rather than requiring every sampled (and possibly illiquid or un-streamable)
    /// instrument to tick. A configured value of zero would let the probe pass with
    /// no observed data, so it is rejected here. The upper bound against the actual
    /// sampled target count is enforced where that count is known (at build time for
    /// configured targets, at metadata-response install time for sampled targets).
    fn validate_readiness_probe_min_observed_targets(
        readiness_probe: &DataClientReadinessProbeBlock,
    ) -> Result<Option<usize>, BoltV3LiveNodeError> {
        match readiness_probe.min_observed_targets {
            Some(0) => Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "clients.<id>.readiness_probe.min_observed_targets must be a positive integer when configured"
                ),
            )),
            other => Ok(other),
        }
    }

    pub(super) fn strategy_free_data_client_readiness_quote_probe_handle(
        loaded: &LoadedBoltV3Config,
        client_key: &str,
    ) -> Result<BoltV3StrategyFreeReferenceQuoteProbeHandle, BoltV3LiveNodeError> {
        let client = loaded.root.clients.get(client_key).ok_or_else(|| {
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                "data-client readiness probe client_key is not configured"
            ))
        })?;
        if client.data.is_none() {
            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                anyhow::anyhow!(
                    "data-client readiness probe requires the selected client to declare [data]"
                ),
            ));
        }
        let Some(readiness_probe) = &client.readiness_probe else {
            return Ok(BoltV3StrategyFreeReferenceQuoteProbeHandle::from_plan(
                Vec::new(),
                BTreeSet::new(),
                DataClientReadinessProbeMarketDataKind::Quote,
                None,
            ));
        };
        let min_observed_targets = validate_readiness_probe_min_observed_targets(readiness_probe)?;
        match readiness_probe.quote_target_source {
            DataClientReadinessProbeQuoteTargetSource::Configured => {
                let Some(quote_targets) = &readiness_probe.quote_targets else {
                    return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                        anyhow::anyhow!(
                            "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
                        ),
                    ));
                };
                if quote_targets.is_empty() {
                    return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                        anyhow::anyhow!(
                            "data-client readiness quote probe requires clients.<id>.readiness_probe.quote_targets"
                        ),
                    ));
                }
                let subscriptions = quote_targets
                    .values()
                    .map(|target| StrategyFreeReferenceQuoteSubscription {
                        data_client_id: ClientId::from(client_key),
                        instrument_id: target.instrument_id,
                    })
                    .collect();
                let (required, ambiguous_instrument_ids) =
                    dedupe_strategy_free_reference_quote_subscriptions(subscriptions);
                if let Some(min_observed) = min_observed_targets
                    && min_observed > required.len()
                {
                    return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                        anyhow::anyhow!(
                            "clients.<id>.readiness_probe.min_observed_targets={min_observed} exceeds the {} configured readiness_probe.quote_targets",
                            required.len()
                        ),
                    ));
                }
                Ok(BoltV3StrategyFreeReferenceQuoteProbeHandle::from_plan(
                    required,
                    ambiguous_instrument_ids,
                    readiness_probe.market_data_kind,
                    min_observed_targets,
                ))
            }
            DataClientReadinessProbeQuoteTargetSource::MetadataResponse => {
                if readiness_probe.market_data_kind == DataClientReadinessProbeMarketDataKind::Trade
                    && readiness_probe.chunk_size.is_some()
                {
                    let chunk_size = match readiness_probe.chunk_size {
                        Some(chunk_size) if chunk_size > 0 => chunk_size,
                        _ => {
                            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                                anyhow::anyhow!(
                                    "trade chunk-count readiness probe requires positive clients.<id>.readiness_probe.chunk_size"
                                ),
                            ));
                        }
                    };
                    let chunk_observation_window_seconds = match readiness_probe
                        .chunk_observation_window_seconds
                    {
                        Some(window) if window > 0 => window,
                        _ => {
                            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                                anyhow::anyhow!(
                                    "trade chunk-count readiness probe requires positive clients.<id>.readiness_probe.chunk_observation_window_seconds"
                                ),
                            ));
                        }
                    };
                    let required_live_markets = match min_observed_targets {
                        Some(required_live_markets) if required_live_markets > 0 => {
                            required_live_markets
                        }
                        _ => {
                            return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                                anyhow::anyhow!(
                                    "trade chunk-count readiness probe requires positive clients.<id>.readiness_probe.min_observed_targets (m)"
                                ),
                            ));
                        }
                    };
                    return Ok(
                    BoltV3StrategyFreeReferenceQuoteProbeHandle::from_metadata_response_chunk_count_plan(
                        ClientId::from(client_key),
                        chunk_size,
                        chunk_observation_window_seconds,
                        required_live_markets,
                        readiness_probe.market_data_kind,
                    ),
                );
                }
                let max_quote_targets = readiness_probe.max_metadata_quote_targets.ok_or_else(|| {
                BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                    "data-client readiness quote probe requires clients.<id>.readiness_probe.max_metadata_quote_targets when quote_target_source = \"metadata_response\""
                ))
            })?;
                if max_quote_targets == 0 {
                    return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                        anyhow::anyhow!(
                            "data-client readiness quote probe requires positive clients.<id>.readiness_probe.max_metadata_quote_targets"
                        ),
                    ));
                }
                let allow_target_sampling = readiness_probe
                .allow_metadata_target_sampling
                .ok_or_else(|| {
                    BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                        "data-client readiness quote probe requires clients.<id>.readiness_probe.allow_metadata_target_sampling when quote_target_source = \"metadata_response\""
                    ))
                })?;
                Ok(
                    BoltV3StrategyFreeReferenceQuoteProbeHandle::from_metadata_response_plan(
                        ClientId::from(client_key),
                        max_quote_targets,
                        allow_target_sampling,
                        readiness_probe.market_data_kind,
                        min_observed_targets,
                    ),
                )
            }
        }
    }

    fn dedupe_strategy_free_reference_quote_subscriptions(
        subscriptions: Vec<StrategyFreeReferenceQuoteSubscription>,
    ) -> (
        Vec<StrategyFreeReferenceQuoteSubscription>,
        BTreeSet<String>,
    ) {
        let mut seen = BTreeSet::new();
        let mut by_instrument: BTreeMap<String, String> = BTreeMap::new();
        let mut ambiguous_instrument_ids = BTreeSet::new();
        let mut deduped = Vec::new();
        for subscription in subscriptions {
            let data_client_id = subscription.data_client_id.to_string();
            let instrument_id = subscription.instrument_id.to_string();
            match by_instrument.get(&instrument_id) {
                Some(existing_data_client_id) if existing_data_client_id != &data_client_id => {
                    ambiguous_instrument_ids.insert(instrument_id.clone());
                }
                None => {
                    by_instrument.insert(instrument_id.clone(), data_client_id.clone());
                }
                _ => {}
            }
            let key = (data_client_id, instrument_id);
            if seen.insert(key) {
                deduped.push(subscription);
            }
        }
        (deduped, ambiguous_instrument_ids)
    }
}

use strategy_free_probe::*;

impl BoltV3StrategyFreeReferenceCacheEvidence {
    pub fn cached_instrument_ids(&self) -> &[String] {
        &self.cached_instrument_ids
    }
}

#[derive(Debug)]
struct NoStrategyDecisionEvidenceWriter;

impl BoltV3DecisionEvidenceWriter for NoStrategyDecisionEvidenceWriter {
    fn record_strategy_input_snapshot(
        &self,
        _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
    ) -> Result<()> {
        Ok(())
    }

    fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> Result<()> {
        Ok(())
    }

    fn record_admission_decision(&self, _decision: &BoltV3AdmissionDecisionEvidence) -> Result<()> {
        Ok(())
    }

    fn record_basket_admission_decision(
        &self,
        _decision: &BoltV3BasketAdmissionDecisionEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_capital_admission_rebuild_audit(
        &self,
        _audit: &BoltV3CapitalAdmissionRebuildAuditEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_submit_reservation_metadata(
        &self,
        _metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_submit_reservation_fill(
        &self,
        _fill: &BoltV3SubmitReservationFillEvidence,
    ) -> Result<()> {
        Ok(())
    }

    fn record_entry_skip(&self, _skip: &BoltV3EntrySkipEvidence) -> Result<()> {
        Ok(())
    }

    fn record_exit_decision(&self, _decision: &BoltV3ExitDecisionEvidence) -> Result<()> {
        Ok(())
    }

    fn record_exit_evaluation(&self, _evidence: &BoltV3ExitEvaluationEvidence) -> Result<()> {
        Ok(())
    }

    fn record_loss_governor_halt(&self, _evidence: &BoltV3LossGovernorHaltEvidence) -> Result<()> {
        Ok(())
    }

    fn record_order_reject(&self, _evidence: &BoltV3OrderRejectEvidence) -> Result<()> {
        Ok(())
    }

    fn record_requote_throttle(&self, _throttle: &BoltV3RequoteThrottleEvidence) -> Result<()> {
        Ok(())
    }
}

struct BoltV3LiveNodeRuntimeFeeds {
    loss_protection: Option<Rc<RefCell<KillSwitchLossProtection>>>,
    loss_halt_action_policy: Option<LossGovernorHaltActionPolicy>,
    loss_runtime_feed: Option<Rc<RefCell<LossGovernorRuntimeFeed>>>,
    loss_runtime_feed_subscription: Option<LossGovernorRuntimeFeedSubscription>,
    order_reject_observer_feed: Option<Arc<Mutex<BoltV3OrderRejectObserverFeed>>>,
    order_reject_observer_feed_subscription: Option<OrderRejectObserverFeedSubscription>,
    capital_admission_runtime_feed: Option<Arc<Mutex<CapitalAdmissionRuntimeFeed>>>,
    capital_admission_runtime_feed_subscription: Option<CapitalAdmissionRuntimeFeedSubscription>,
    capital_admission_venue_spendability_source:
        Option<BoltV3CapitalAdmissionVenueSpendabilitySourceConfig>,
    submit_reservation_recovery: Option<BoltV3SubmitReservationRecoveryConfig>,
}

impl BoltV3LiveNodeRuntime {
    fn new(
        node: LiveNode,
        registration_summary: BoltV3RegistrationSummary,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
        feeds: BoltV3LiveNodeRuntimeFeeds,
        iv_runtime: Option<IvRuntimeEngine>,
        iv_event_bindings: Option<BoltV3IvRuntimeEventBindings>,
        redaction_values: Vec<Zeroizing<String>>,
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
            capital_admission_runtime_feed_subscription: feeds
                .capital_admission_runtime_feed_subscription,
            capital_admission_venue_spendability_source: feeds
                .capital_admission_venue_spendability_source,
            submit_reservation_recovery: feeds.submit_reservation_recovery,
            iv_runtime,
            iv_event_bindings,
            redaction_values,
        }
    }

    pub fn registered_strategy_ids(&self) -> Vec<StrategyId> {
        self.node.kernel().trader().borrow().strategy_ids()
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

    pub fn order_reject_observer_feed_configured(&self) -> bool {
        self.order_reject_observer_feed.is_some()
            && self.order_reject_observer_feed_subscription.is_some()
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
            && self.capital_admission_runtime_feed_subscription.is_some()
    }

    pub fn refresh_capital_admission_venue_spendability_from_configured_source(
        &self,
    ) -> Result<Option<BoltV3SubmitCapitalAdmissionNtComponents>, BoltV3LiveNodeError> {
        let Some(config) = self.capital_admission_venue_spendability_source.as_ref() else {
            return Ok(None);
        };
        let Some(feed) = self.capital_admission_runtime_feed.as_ref() else {
            return Err(BoltV3LiveNodeError::Build(anyhow::anyhow!(
                "capital admission venue spendability source configured without runtime feed"
            )));
        };
        refresh_capital_admission_venue_spendability_from_source(feed, config)
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
    pub fn rebuild_capital_admission_from_nt_cache(
        &self,
        now_ns: u64,
    ) -> BoltV3SubmitCapitalAdmissionRebuildDecision {
        let (account_id, binary_instrument_ids, collateral_currency) =
            match self.capital_admission_runtime_feed.as_ref() {
                Some(feed) => {
                    let feed = feed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    (
                        Some(feed.configured_account_id()),
                        feed.configured_binary_instrument_ids(),
                        Some(feed.configured_collateral_currency()),
                    )
                }
                None => (None, None, None),
            };
        let cache = self.node.kernel().cache();
        let cache = cache.borrow();
        let open_order_snapshots = match account_id.as_ref() {
            Some(account_id) => cache
                .orders_open(None, None, None, Some(account_id), None)
                .into_iter()
                .map(|order| order.cloned())
                .collect::<Vec<_>>(),
            None => cache
                .orders_open(None, None, None, None, None)
                .into_iter()
                .map(|order| order.cloned())
                .collect::<Vec<_>>(),
        };
        let open_client_order_ids = open_order_snapshots
            .iter()
            .map(|order| order.client_order_id().to_string())
            .collect::<Vec<_>>();
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

        let account_cache_is_authoritative = cached_account_balances.is_some();
        // Seed live NT order and position state before rebuilding reservations from the same snapshot.
        if let Some(feed) = self.capital_admission_runtime_feed.as_ref() {
            let mut feed = feed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some((free_collateral, total_equity)) = cached_account_balances {
                feed.seed_account_portfolio_snapshot(free_collateral, total_equity, now_ns);
            }
            if account_cache_is_authoritative || !open_client_order_ids.is_empty() {
                feed.seed_cache_snapshot(
                    open_client_order_ids.clone(),
                    yes_position,
                    no_position,
                    now_ns,
                );
            }
        }

        let recovered_reservations = if open_order_snapshots.is_empty() {
            None
        } else {
            self.submit_reservation_recovery
                .as_ref()
                .and_then(|config| {
                    match read_submit_reservation_recovery_evidence(&config.path, config.max_bytes)
                    {
                        Ok(recovery) => Some(recovery),
                        Err(error) => {
                            log::warn!(
                                "bolt-v3 submit admission could not recover Bolt reservation metadata from decision evidence: {error:#}"
                            );
                            None
                        }
                    }
                })
        };
        let mut reservations = Vec::with_capacity(open_order_snapshots.len());
        let mut all_open_orders_attributed =
            open_order_snapshots.is_empty() || recovered_reservations.is_some();
        for order in &open_order_snapshots {
            let Some(recovered_reservations) = recovered_reservations.as_ref() else {
                all_open_orders_attributed = false;
                break;
            };
            let Some(evidence) = nt_open_order_evidence_from_order(order, now_ns) else {
                all_open_orders_attributed = false;
                break;
            };
            let Some(recovered) = recovered_reservations
                .metadata_by_client_order_id
                .get(&evidence.client_order_id)
            else {
                all_open_orders_attributed = false;
                break;
            };
            let Some(reservation) = self
                .submit_admission
                .capital_admission_open_order_reservation_from_known_metadata(evidence, recovered)
            else {
                all_open_orders_attributed = false;
                break;
            };
            reservations.push(reservation);
        }
        if !all_open_orders_attributed {
            reservations.clear();
        }

        let mut rebuild = self
            .submit_admission
            .rebuild_capital_admission_open_order_snapshot(
                BoltV3SubmitCapitalAdmissionOpenOrderSnapshot {
                    observed_at_ns: now_ns,
                    evidence_label: "nt_open_order_cache".to_string(),
                    observed_open_order_count: open_order_snapshots.len(),
                    all_open_orders_attributed,
                    reservations,
                },
                now_ns,
            );
        if let Some(missing) = missing_nt_account_cache_balance {
            rebuild = rebuild.with_missing_nt_account_cache_balance(
                missing.account_id,
                missing.collateral_currency,
            );
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
    /// The bolt-v3 controlled-connect boundary
    /// ([`connect_bolt_v3_clients`]) bounds the dispatched
    /// `NautilusKernel::connect_data_clients` and
    /// `NautilusKernel::connect_exec_clients` calls by the
    /// `nautilus.timeout_connection_secs` value from the loaded
    /// bolt-v3 config. A `ConnectTimeout` is surfaced when that bound
    /// elapses before NT's engine-level connect dispatchers return,
    /// instead of the controlled-connect call hanging indefinitely.
    /// The wrapped value is the configured timeout the boundary
    /// applied (in seconds), captured so log/audit consumers can
    /// distinguish a 1-second test timeout from a 30-second
    /// production timeout without re-reading the source config.
    ConnectTimeout {
        timeout_secs: u64,
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
    /// submit-reservation metadata, so submit admission would arm with an
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
            BoltV3LiveNodeError::ConnectTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 controlled-connect exceeded the configured \
                 nautilus.timeout_connection_secs bound ({timeout_secs}s)"
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
            BoltV3LiveNodeError::ConnectTimeout { .. }
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

pub fn build_bolt_v3_live_node_with_resolved(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    // RV source-client validation is owned by the strategy-registration
    // chokepoint; trade transport must retain the clients it will validate.
    let transport_loaded =
        trade_transport_loaded_config(loaded, RealizedVolatilityTransportScope::Subscribed)?;
    check_no_forbidden_credential_env_vars(&transport_loaded.root)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    build_bolt_v3_live_node_from_resolved_transport(&transport_loaded, resolved)
}

fn build_bolt_v3_live_node_from_resolved_transport(
    transport_loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let bundle =
        live_node_adapter_bundle_with_provider_live_submit_approvals(transport_loaded, resolved)?;
    let (runtime, _summary) = build_live_node_with_clients_and_submit_approval_limits(
        transport_loaded,
        resolved,
        bundle.configs,
        bundle.live_submit_approval_limits,
    )?;
    Ok(runtime)
}

fn resolve_bolt_v3_live_node_secrets(
    loaded: &LoadedBoltV3Config,
) -> Result<ResolvedBoltV3Secrets, BoltV3LiveNodeError> {
    check_no_forbidden_credential_env_vars(&loaded.root)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    // Per #252 design review: own the resolver session at the bolt-v3
    // startup boundary so a single AWS SDK config + SsmClient cache covers
    // every secret resolution in this build, and so the session lifetime is
    // visible to the caller of `resolve_bolt_v3_secrets`. Session-setup
    // failure surfaces as the dedicated `SecretResolverSetup` variant
    // (#255-2) so operator-facing messages don't pretend a venue or SSM
    // path is involved before any path has been read.
    let session = SsmResolverSession::new().map_err(BoltV3LiveNodeError::SecretResolverSetup)?;
    resolve_bolt_v3_secrets(&session, loaded).map_err(BoltV3LiveNodeError::SecretResolution)
}

fn live_node_adapter_bundle_with_provider_live_submit_approvals(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3LiveNodeAdapterBundle, BoltV3LiveNodeError> {
    if configured_provider_live_submit_client_count(loaded)? == 0 {
        return Ok(BoltV3LiveNodeAdapterBundle {
            configs: map_bolt_v3_adapters(loaded, resolved)
                .map_err(BoltV3LiveNodeError::AdapterMapping)?,
            live_submit_approval_limits: BTreeMap::new(),
        });
    }
    let build_head_sha = current_build_head_sha().ok_or_else(|| {
        BoltV3LiveNodeError::OperatorApprovalConsumption(anyhow::anyhow!(
            "bolt-v3 build head_sha is unavailable or invalid"
        ))
    })?;
    let now_unix_seconds = current_unix_seconds_u64()?;
    live_node_adapter_bundle_with_provider_approvals_at(
        loaded,
        resolved,
        now_unix_seconds,
        build_head_sha,
    )
}

fn live_node_adapter_bundle_with_provider_approvals_at(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    now_unix_seconds: u64,
    build_head_sha: &str,
) -> Result<BoltV3LiveNodeAdapterBundle, BoltV3LiveNodeError> {
    let approvals = load_provider_live_submit_approvals_for_live_node(
        loaded,
        resolved,
        now_unix_seconds,
        build_head_sha,
    )?;
    if approvals.is_empty() {
        return Ok(BoltV3LiveNodeAdapterBundle {
            configs: map_bolt_v3_adapters(loaded, resolved)
                .map_err(BoltV3LiveNodeError::AdapterMapping)?,
            live_submit_approval_limits: BTreeMap::new(),
        });
    }
    let configs = map_bolt_v3_adapters_with_runtime_approvals(
        loaded,
        resolved,
        ProviderRuntimeApprovals {
            live_submit: Some(&approvals),
        },
    )
    .map_err(BoltV3LiveNodeError::AdapterMapping)?;
    Ok(BoltV3LiveNodeAdapterBundle {
        configs,
        live_submit_approval_limits: live_submit_approval_limits_for_submit_admission(&approvals),
    })
}

fn live_submit_approval_limits_for_submit_admission(
    approvals: &ProviderLiveSubmitApprovals,
) -> BTreeMap<String, BoltV3LiveSubmitApprovalLimits> {
    approvals
        .order_limits()
        .map(|(client_key, order_limits)| {
            (
                client_key.clone(),
                BoltV3LiveSubmitApprovalLimits {
                    max_order_count: order_limits.max_order_count,
                    max_order_notional: order_limits.max_order_notional,
                },
            )
        })
        .collect()
}

fn configured_provider_live_submit_client_count(
    loaded: &LoadedBoltV3Config,
) -> Result<usize, BoltV3LiveNodeError> {
    let mut count = 0;
    for client in loaded.root.clients.values() {
        let Some(binding) = bolt_v3_providers::binding_for_provider_key(client.venue.as_str())
        else {
            continue;
        };
        if binding.load_live_submit_approval.is_some() && client.execution.is_some() {
            count += 1;
        }
    }
    Ok(count)
}

fn load_provider_live_submit_approvals_for_live_node(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    now_unix_seconds: u64,
    build_head_sha: &str,
) -> Result<ProviderLiveSubmitApprovals, BoltV3LiveNodeError> {
    let mut approvals = ProviderLiveSubmitApprovals::empty();
    for (client_key, client) in &loaded.root.clients {
        let Some(binding) = bolt_v3_providers::binding_for_provider_key(client.venue.as_str())
        else {
            continue;
        };
        let Some(load_live_submit_approval) = binding.load_live_submit_approval else {
            continue;
        };
        if let Some(approval) = load_live_submit_approval(ProviderLiveSubmitApprovalContext {
            loaded,
            client_key,
            client,
            resolved,
            product_surface: None,
            now_unix_seconds,
            build_head_sha,
        })
        .map_err(BoltV3LiveNodeError::OperatorApprovalConsumption)?
        {
            approvals.insert(client_key.clone(), approval);
        }
    }
    Ok(approvals)
}

fn current_unix_seconds_u64() -> Result<u64, BoltV3LiveNodeError> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| {
            BoltV3LiveNodeError::OperatorApprovalConsumption(anyhow::Error::new(source))
        })?
        .as_secs())
}

fn current_unix_nanos() -> Result<u64> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    u64::try_from(nanos).map_err(|_| anyhow::anyhow!("current unix nanoseconds exceed u64"))
}

pub fn build_bolt_v3_strategy_free_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let transport_loaded =
        trade_transport_loaded_config(loaded, RealizedVolatilityTransportScope::NotSubscribed)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&transport_loaded)?;
    build_bolt_v3_strategy_free_live_node_from_resolved_transport(&transport_loaded, &resolved)
}

pub fn build_bolt_v3_strategy_free_live_node_with_resolved(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let transport_loaded =
        trade_transport_loaded_config(loaded, RealizedVolatilityTransportScope::NotSubscribed)?;
    check_no_forbidden_credential_env_vars(&transport_loaded.root)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    build_bolt_v3_strategy_free_live_node_from_resolved_transport(&transport_loaded, resolved)
}

fn build_bolt_v3_strategy_free_live_node_from_resolved_transport(
    transport_loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let adapters = strategy_free_transport_adapter_configs(transport_loaded, resolved)?;
    let strategy_free_loaded = strategy_free_transport_loaded_config(transport_loaded);
    let (runtime, _summary) =
        build_live_node_with_clients(&strategy_free_loaded, resolved, adapters)?;
    Ok(runtime)
}

pub fn build_bolt_v3_strategy_free_live_node_with_summary<F, R, E>(
    loaded: &LoadedBoltV3Config,
    env_is_set: F,
    resolver: R,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError>
where
    F: FnMut(&str) -> bool,
    R: FnMut(&str, &str) -> Result<String, E>,
    E: std::fmt::Display,
{
    let transport_loaded =
        trade_transport_loaded_config(loaded, RealizedVolatilityTransportScope::NotSubscribed)?;
    check_no_forbidden_credential_env_vars_with(&transport_loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let resolved = resolve_bolt_v3_secrets_with(&transport_loaded, resolver)
        .map_err(BoltV3LiveNodeError::SecretResolution)?;
    let adapters = strategy_free_transport_adapter_configs(&transport_loaded, &resolved)?;
    let strategy_free_loaded = strategy_free_transport_loaded_config(&transport_loaded);
    build_live_node_with_clients(&strategy_free_loaded, &resolved, adapters)
}

pub fn build_bolt_v3_strategy_free_data_client_probe_live_node(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<(BoltV3LiveNodeRuntime, LoadedBoltV3Config), BoltV3LiveNodeError> {
    let probe_loaded = data_client_probe_loaded_config(loaded, client_key)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&probe_loaded)?;
    let adapters = strategy_free_transport_adapter_configs(&probe_loaded, &resolved)?;
    let strategy_free_loaded = strategy_free_transport_loaded_config(&probe_loaded);
    let (runtime, _summary) =
        build_live_node_with_clients(&strategy_free_loaded, &resolved, adapters)?;
    Ok((runtime, strategy_free_loaded))
}

/// Run an already-built strategy-free data-client probe node.
///
/// The caller must build `runtime` at a synchronous startup boundary before
/// entering Tokio, because the build path owns SSM resolution through
/// `SsmResolverSession`.
pub async fn run_bolt_v3_data_client_probe(
    mut runtime: BoltV3LiveNodeRuntime,
    probe_loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<BoltV3DataClientProbeReport, BoltV3LiveNodeError> {
    let handle = strategy_free_data_client_readiness_quote_probe_handle(probe_loaded, client_key)?;
    let readiness_probe = probe_loaded
        .root
        .clients
        .get(client_key)
        .and_then(|client| client.readiness_probe.as_ref())
        .ok_or_else(|| {
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                "data-client readiness probe requires clients.<id>.readiness_probe"
            ))
        })?;
    let market_data_kind = readiness_probe.market_data_kind;
    let book_type = readiness_probe
        .book_type
        .map(readiness_probe_book_type_to_nt);
    let quote_target_source = readiness_probe.quote_target_source;
    let client_venue = probe_loaded
        .root
        .clients
        .get(client_key)
        .map(|client| client.venue)
        .ok_or_else(|| {
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                "data-client readiness probe client_key is not configured"
            ))
        })?;

    let mut subscribed = Vec::new();
    let mut observer = None;
    let mut metadata_observer = None;
    let mut metadata_driver = None;

    match quote_target_source {
        DataClientReadinessProbeQuoteTargetSource::Configured => {
            let subscriptions =
                strategy_free_configured_data_client_probe_subscriptions(probe_loaded, client_key)?;
            for subscription in &subscriptions {
                if let Err(error) = subscribe_strategy_free_probe_subscription(
                    &mut runtime,
                    subscription,
                    market_data_kind,
                    book_type,
                ) {
                    for previous in subscribed.iter().rev() {
                        unsubscribe_strategy_free_probe_subscription(
                            &mut runtime,
                            previous,
                            market_data_kind,
                        );
                    }
                    return Err(error);
                }
                subscribed.push(subscription.clone());
            }
            observer = Some(StrategyFreeDataClientProbeObserver::register(
                &handle,
                &subscriptions,
                runtime.handle(),
            ));
        }
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse => {
            if handle.is_chunk_count_mode() {
                return Err(BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(
                    anyhow::anyhow!(
                        "ops data-client-probe does not support trade chunk-count metadata_response probes"
                    ),
                ));
            }
            runtime.ensure_strategy_free_data_client_registered(
                ClientId::from(client_key),
                readiness_probe_market_data_kind_label(market_data_kind),
            )?;
            let metadata = StrategyFreeMetadataResponseProbeObserver::register(
                &handle,
                client_venue,
                market_data_kind,
                book_type,
                runtime.handle(),
            )?;
            metadata_driver = Some(metadata.driver());
            metadata_observer = Some(metadata);
        }
    }

    let stop_handle = runtime.handle();
    let run_timeout = Duration::from_secs(strategy_free_start_timeout_secs(probe_loaded)?);
    let stop_timeout = Duration::from_secs(strategy_free_stop_timeout_secs(probe_loaded)?);
    let (run_result, driver_error) = if let Some(driver) = metadata_driver {
        let run_future = runtime.run_strategy_free_until_stop_or_timeout(run_timeout, stop_timeout);
        tokio::pin!(run_future);
        let driver_future = driver.drive_until_subscribed();
        tokio::pin!(driver_future);
        let mut driver_result = None;
        let run_result = loop {
            tokio::select! {
                result = &mut run_future => break result,
                result = &mut driver_future, if driver_result.is_none() => {
                    if result.is_err() {
                        stop_handle.stop();
                    }
                    driver_result = Some(result);
                }
            }
        };
        (run_result, driver_result.and_then(Result::err))
    } else {
        (
            runtime
                .run_strategy_free_until_stop_or_timeout(run_timeout, stop_timeout)
                .await,
            None,
        )
    };

    for subscription in subscribed.iter().rev() {
        unsubscribe_strategy_free_probe_subscription(&mut runtime, subscription, market_data_kind);
    }
    if let Some(metadata) = metadata_observer {
        for subscription in metadata.subscriptions().iter().rev() {
            unsubscribe_strategy_free_probe_subscription(
                &mut runtime,
                subscription,
                market_data_kind,
            );
        }
        metadata.unregister();
    }
    if let Some(observer) = observer {
        observer.unregister();
    }

    if let Some(error) = driver_error {
        return Err(error);
    }
    let run_timed_out = run_result?;
    if handle.has_all_required_market_data() {
        return Ok(BoltV3DataClientProbeReport {
            client_key: client_key.to_string(),
            market_data_kind: readiness_probe_market_data_kind_label(market_data_kind).to_string(),
            required_observation_count: handle.required_market_data_count(),
            observed_update_count: handle.observed_market_data_count(),
        });
    }

    let reason = handle.failure_error().unwrap_or_else(|| {
        let observed = handle.observed_market_data_count();
        let required = handle.required_market_data_count();
        if run_timed_out {
            format!(
                "timed out before observing required data-client market data ({observed}/{required} observed)"
            )
        } else {
            format!(
                "live node exited before observing required data-client market data ({observed}/{required} observed)"
            )
        }
    });
    Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed { reason })
}

pub async fn run_bolt_v3_data_client_census(
    mut runtime: BoltV3LiveNodeRuntime,
    census_loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<BoltV3DataClientCensusReport, BoltV3LiveNodeError> {
    let client = census_loaded.root.clients.get(client_key).ok_or_else(|| {
        BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client census client_key is not configured".to_string(),
        }
    })?;
    if client.data.is_none() {
        return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client census requires the selected client to declare [data]".to_string(),
        });
    }
    runtime.ensure_strategy_free_data_client_registered(
        ClientId::from(client_key),
        "instrument census",
    )?;

    let start_timeout = Duration::from_secs(strategy_free_start_timeout_secs(census_loaded)?);
    let stop_timeout = Duration::from_secs(strategy_free_stop_timeout_secs(census_loaded)?);
    let poll_interval = Duration::from_millis(
        census_loaded
            .root
            .persistence
            .runtime_capture_start_poll_interval_ms,
    );
    runtime
        .run_strategy_free_until_running_then_stop(start_timeout, stop_timeout, poll_interval)
        .await?;
    data_client_census_report(client_key, runtime.cached_instrument_ids())
}

fn data_client_census_report(
    client_key: &str,
    mut instrument_ids: Vec<String>,
) -> Result<BoltV3DataClientCensusReport, BoltV3LiveNodeError> {
    instrument_ids.sort();
    instrument_ids.dedup();
    if instrument_ids.is_empty() {
        return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client census observed zero cached instruments".to_string(),
        });
    }
    Ok(BoltV3DataClientCensusReport {
        client_key: client_key.to_string(),
        cached_instrument_count: instrument_ids.len(),
        cached_instrument_ids_sha256: instrument_ids_sha256(&instrument_ids),
    })
}

fn instrument_ids_sha256(instrument_ids: &[String]) -> String {
    let mut hasher = Sha256::new();
    for instrument_id in instrument_ids {
        hasher.update(instrument_id.as_bytes());
        hasher.update(b"\0");
    }
    hex::encode(hasher.finalize())
}

enum StrategyFreeDataClientProbeHandler {
    Quote(MStr<Pattern>, TypedHandler<QuoteTick>),
    Book(MStr<Pattern>, TypedHandler<OrderBookDeltas>),
    Trade(MStr<Pattern>, TypedHandler<TradeTick>),
}

struct StrategyFreeDataClientProbeObserver {
    handlers: Vec<StrategyFreeDataClientProbeHandler>,
}

impl StrategyFreeDataClientProbeObserver {
    fn register(
        handle: &BoltV3StrategyFreeReferenceQuoteProbeHandle,
        subscriptions: &[StrategyFreeReferenceQuoteSubscription],
        stop_handle: LiveNodeHandle,
    ) -> Self {
        let mut handlers = Vec::new();
        for subscription in subscriptions {
            match handle.market_data_kind {
                DataClientReadinessProbeMarketDataKind::Quote => {
                    let probe_handle = handle.clone();
                    let stop_handle = stop_handle.clone();
                    let pattern: MStr<Pattern> =
                        switchboard::get_quotes_topic(subscription.instrument_id).into();
                    let handler = TypedHandler::from(move |quote: &QuoteTick| {
                        probe_handle.record_quote(
                            quote,
                            get_atomic_clock_realtime().get_time_ns().as_u64(),
                        );
                        if probe_handle.has_all_required_market_data() {
                            stop_handle.stop();
                        }
                    });
                    msgbus::subscribe_quotes(pattern, handler.clone(), None);
                    handlers.push(StrategyFreeDataClientProbeHandler::Quote(pattern, handler));
                }
                DataClientReadinessProbeMarketDataKind::Book => {
                    let probe_handle = handle.clone();
                    let stop_handle = stop_handle.clone();
                    let pattern: MStr<Pattern> =
                        switchboard::get_book_deltas_topic(subscription.instrument_id).into();
                    let handler = TypedHandler::from(move |deltas: &OrderBookDeltas| {
                        probe_handle.record_book_deltas(
                            deltas,
                            get_atomic_clock_realtime().get_time_ns().as_u64(),
                        );
                        if probe_handle.has_all_required_market_data() {
                            stop_handle.stop();
                        }
                    });
                    msgbus::subscribe_book_deltas(pattern, handler.clone(), None);
                    handlers.push(StrategyFreeDataClientProbeHandler::Book(pattern, handler));
                }
                DataClientReadinessProbeMarketDataKind::Trade => {
                    let probe_handle = handle.clone();
                    let stop_handle = stop_handle.clone();
                    let pattern: MStr<Pattern> =
                        switchboard::get_trades_topic(subscription.instrument_id).into();
                    let handler = TypedHandler::from(move |trade: &TradeTick| {
                        probe_handle.record_trade(trade);
                        if probe_handle.has_all_required_market_data() {
                            stop_handle.stop();
                        }
                    });
                    msgbus::subscribe_trades(pattern, handler.clone(), None);
                    handlers.push(StrategyFreeDataClientProbeHandler::Trade(pattern, handler));
                }
            }
        }
        Self { handlers }
    }

    fn unregister(self) {
        for handler in self.handlers {
            match handler {
                StrategyFreeDataClientProbeHandler::Quote(pattern, handler) => {
                    msgbus::unsubscribe_quotes(pattern, &handler);
                }
                StrategyFreeDataClientProbeHandler::Book(pattern, handler) => {
                    msgbus::unsubscribe_book_deltas(pattern, &handler);
                }
                StrategyFreeDataClientProbeHandler::Trade(pattern, handler) => {
                    msgbus::unsubscribe_trades(pattern, &handler);
                }
            }
        }
    }
}

#[derive(Clone)]
struct StrategyFreeMetadataResponseProbeDriver {
    state: Rc<StrategyFreeMetadataResponseProbeState>,
}

impl StrategyFreeMetadataResponseProbeDriver {
    async fn drive_until_subscribed(&self) -> Result<(), BoltV3LiveNodeError> {
        loop {
            if self.state.has_subscriptions() {
                return Ok(());
            }
            if self.state.instrument_count() >= self.state.max_metadata_quote_targets {
                return self.state.install_and_subscribe();
            }
            self.state.notify.notified().await;
        }
    }
}

struct StrategyFreeMetadataResponseProbeObserver {
    pattern: MStr<Pattern>,
    handler: TypedHandler<InstrumentAny>,
    state: Rc<StrategyFreeMetadataResponseProbeState>,
}

impl StrategyFreeMetadataResponseProbeObserver {
    fn register(
        handle: &BoltV3StrategyFreeReferenceQuoteProbeHandle,
        venue: Venue,
        market_data_kind: DataClientReadinessProbeMarketDataKind,
        book_type: Option<BookType>,
        stop_handle: LiveNodeHandle,
    ) -> Result<Self, BoltV3LiveNodeError> {
        let max_metadata_quote_targets =
            handle.metadata_response_max_quote_targets.ok_or_else(|| {
                BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                    "data-client readiness probe requires clients.<id>.readiness_probe.max_metadata_quote_targets when quote_target_source = \"metadata_response\""
                ))
            })?;
        let state = Rc::new(StrategyFreeMetadataResponseProbeState {
            handle: handle.clone(),
            venue,
            market_data_kind,
            book_type,
            max_metadata_quote_targets,
            instruments: RefCell::new(BTreeMap::new()),
            subscriptions: RefCell::new(Vec::new()),
            market_observer: RefCell::new(None),
            notify: tokio::sync::Notify::new(),
            stop_handle,
        });
        let handler_state = state.clone();
        let handler = TypedHandler::from(move |instrument: &InstrumentAny| {
            let instrument_id = instrument.id();
            if instrument_id.venue != handler_state.venue {
                return;
            }
            let mut instruments = handler_state.instruments.borrow_mut();
            let previous_len = instruments.len();
            instruments.insert(instrument_id.to_string(), instrument_id);
            if instruments.len() != previous_len
                && instruments.len() >= handler_state.max_metadata_quote_targets
            {
                handler_state.notify.notify_one();
            }
        });
        let pattern = crate::bolt_v3_instrument_metadata_bus::metadata_instrument_pattern(venue);
        crate::bolt_v3_instrument_metadata_bus::attach_metadata_instrument_handler(
            pattern,
            handler.clone(),
        );
        Ok(Self {
            pattern,
            handler,
            state,
        })
    }

    fn driver(&self) -> StrategyFreeMetadataResponseProbeDriver {
        StrategyFreeMetadataResponseProbeDriver {
            state: self.state.clone(),
        }
    }

    fn subscriptions(&self) -> Vec<StrategyFreeReferenceQuoteSubscription> {
        self.state.subscriptions.borrow().clone()
    }

    fn unregister(self) {
        crate::bolt_v3_instrument_metadata_bus::detach_metadata_instrument_handler(
            self.pattern,
            &self.handler,
        );
        if let Some(observer) = self.state.market_observer.borrow_mut().take() {
            observer.unregister();
        }
    }
}

struct StrategyFreeMetadataResponseProbeState {
    handle: BoltV3StrategyFreeReferenceQuoteProbeHandle,
    venue: Venue,
    market_data_kind: DataClientReadinessProbeMarketDataKind,
    book_type: Option<BookType>,
    max_metadata_quote_targets: usize,
    instruments: RefCell<BTreeMap<String, InstrumentId>>,
    subscriptions: RefCell<Vec<StrategyFreeReferenceQuoteSubscription>>,
    market_observer: RefCell<Option<StrategyFreeDataClientProbeObserver>>,
    notify: tokio::sync::Notify,
    stop_handle: LiveNodeHandle,
}

impl StrategyFreeMetadataResponseProbeState {
    fn instrument_count(&self) -> usize {
        self.instruments.borrow().len()
    }

    fn has_subscriptions(&self) -> bool {
        !self.subscriptions.borrow().is_empty()
    }

    fn install_and_subscribe(&self) -> Result<(), BoltV3LiveNodeError> {
        if self.has_subscriptions() {
            return Ok(());
        }
        let instrument_ids = self
            .instruments
            .borrow()
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let subscriptions = self
            .handle
            .install_metadata_response_instrument_ids(instrument_ids);
        if subscriptions.is_empty() {
            let reason = self.handle.failure_error().unwrap_or_else(|| {
                "metadata_response readiness probe produced no source-owned instrument targets"
                    .to_string()
            });
            return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed { reason });
        }
        let market_observer = StrategyFreeDataClientProbeObserver::register(
            &self.handle,
            &subscriptions,
            self.stop_handle.clone(),
        );
        for subscription in &subscriptions {
            send_strategy_free_probe_subscription(
                subscription,
                self.market_data_kind,
                self.book_type,
            )?;
        }
        *self.subscriptions.borrow_mut() = subscriptions;
        *self.market_observer.borrow_mut() = Some(market_observer);
        Ok(())
    }
}

fn strategy_free_configured_data_client_probe_subscriptions(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<Vec<StrategyFreeReferenceQuoteSubscription>, BoltV3LiveNodeError> {
    let client = loaded.root.clients.get(client_key).ok_or_else(|| {
        BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
            "data-client readiness probe client_key is not configured"
        ))
    })?;
    let readiness_probe = client.readiness_probe.as_ref().ok_or_else(|| {
        BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
            "data-client readiness probe requires clients.<id>.readiness_probe"
        ))
    })?;

    match readiness_probe.quote_target_source {
        DataClientReadinessProbeQuoteTargetSource::Configured => {
            strategy_free_data_client_readiness_quote_subscription_plan(loaded, client_key)
                .map(|(subscriptions, _)| subscriptions)
        }
        DataClientReadinessProbeQuoteTargetSource::MetadataResponse => Err(
            BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                "configured data-client probe subscription planning requires quote_target_source = \"configured\""
            )),
        ),
    }
}

fn send_strategy_free_probe_subscription(
    subscription: &StrategyFreeReferenceQuoteSubscription,
    market_data_kind: DataClientReadinessProbeMarketDataKind,
    book_type: Option<BookType>,
) -> Result<(), BoltV3LiveNodeError> {
    let ts_init = get_atomic_clock_realtime().get_time_ns();
    let sender = get_data_cmd_sender();
    match market_data_kind {
        DataClientReadinessProbeMarketDataKind::Quote => {
            let command = SubscribeQuotes::new(
                subscription.instrument_id,
                Some(subscription.data_client_id),
                None,
                UUID4::new(),
                ts_init,
                None,
                None,
            );
            sender.execute(DataCommand::Subscribe(SubscribeCommand::Quotes(command)));
        }
        DataClientReadinessProbeMarketDataKind::Book => {
            let command = SubscribeBookDeltas::new(
                subscription.instrument_id,
                book_type.ok_or_else(|| {
                    BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                        "data-client readiness book probe requires clients.<id>.readiness_probe.book_type"
                    ))
                })?,
                Some(subscription.data_client_id),
                None,
                UUID4::new(),
                ts_init,
                None,
                false,
                None,
                None,
            );
            sender.execute(DataCommand::Subscribe(SubscribeCommand::BookDeltas(
                command,
            )));
        }
        DataClientReadinessProbeMarketDataKind::Trade => {
            let command = SubscribeTrades::new(
                subscription.instrument_id,
                Some(subscription.data_client_id),
                None,
                UUID4::new(),
                ts_init,
                None,
                None,
            );
            sender.execute(DataCommand::Subscribe(SubscribeCommand::Trades(command)));
        }
    }
    Ok(())
}

fn subscribe_strategy_free_probe_subscription(
    runtime: &mut BoltV3LiveNodeRuntime,
    subscription: &StrategyFreeReferenceQuoteSubscription,
    market_data_kind: DataClientReadinessProbeMarketDataKind,
    book_type: Option<BookType>,
) -> Result<(), BoltV3LiveNodeError> {
    match market_data_kind {
        DataClientReadinessProbeMarketDataKind::Quote => runtime.subscribe_strategy_free_quotes(
            subscription.data_client_id,
            subscription.instrument_id,
        ),
        DataClientReadinessProbeMarketDataKind::Book => runtime.subscribe_strategy_free_book_deltas(
            subscription.data_client_id,
            subscription.instrument_id,
            book_type.ok_or_else(|| {
                BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(anyhow::anyhow!(
                    "data-client readiness book probe requires clients.<id>.readiness_probe.book_type"
                ))
            })?,
        ),
        DataClientReadinessProbeMarketDataKind::Trade => runtime.subscribe_strategy_free_trades(
            subscription.data_client_id,
            subscription.instrument_id,
        ),
    }
}

fn unsubscribe_strategy_free_probe_subscription(
    runtime: &mut BoltV3LiveNodeRuntime,
    subscription: &StrategyFreeReferenceQuoteSubscription,
    market_data_kind: DataClientReadinessProbeMarketDataKind,
) {
    match market_data_kind {
        DataClientReadinessProbeMarketDataKind::Quote => runtime.unsubscribe_strategy_free_quotes(
            subscription.data_client_id,
            subscription.instrument_id,
        ),
        DataClientReadinessProbeMarketDataKind::Book => runtime
            .unsubscribe_strategy_free_book_deltas(
                subscription.data_client_id,
                subscription.instrument_id,
            ),
        DataClientReadinessProbeMarketDataKind::Trade => runtime.unsubscribe_strategy_free_trades(
            subscription.data_client_id,
            subscription.instrument_id,
        ),
    }
}

fn readiness_probe_book_type_to_nt(book_type: DataClientReadinessProbeBookType) -> BookType {
    match book_type {
        DataClientReadinessProbeBookType::L1Mbp => BookType::L1_MBP,
        DataClientReadinessProbeBookType::L2Mbp => BookType::L2_MBP,
        DataClientReadinessProbeBookType::L3Mbo => BookType::L3_MBO,
    }
}

fn readiness_probe_market_data_kind_label(
    market_data_kind: DataClientReadinessProbeMarketDataKind,
) -> &'static str {
    match market_data_kind {
        DataClientReadinessProbeMarketDataKind::Quote => "quote",
        DataClientReadinessProbeMarketDataKind::Book => "book",
        DataClientReadinessProbeMarketDataKind::Trade => "trade",
    }
}

fn strategy_free_transport_adapter_configs(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3AdapterConfigs, BoltV3LiveNodeError> {
    map_bolt_v3_adapters(loaded, resolved).map_err(BoltV3LiveNodeError::AdapterMapping)
}

fn data_client_probe_loaded_config(
    loaded: &LoadedBoltV3Config,
    client_key: &str,
) -> Result<LoadedBoltV3Config, BoltV3LiveNodeError> {
    if client_key.trim().is_empty() {
        return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client probe client_key is not configured".to_string(),
        });
    }
    let client = loaded
        .root
        .clients
        .get(client_key)
        .cloned()
        .ok_or_else(|| BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client probe client_key is not configured".to_string(),
        })?;
    if client.data.is_none() {
        return Err(BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed {
            reason: "data-client probe requires the selected client to declare [data]".to_string(),
        });
    }
    let mut probe_loaded = loaded.clone();
    probe_loaded
        .root
        .clients
        .retain(|configured_key, _| configured_key == client_key);
    probe_loaded
        .strategies
        .retain(|strategy| strategy.config.execution_client_id == ClientId::from(client_key));
    Ok(probe_loaded)
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
pub async fn run_bolt_v3_live_node(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let startup_rebuild_observed_at_ns =
        current_unix_nanos().map_err(BoltV3LiveNodeError::Build)?;
    let startup_rebuild =
        runtime.rebuild_capital_admission_from_nt_cache(startup_rebuild_observed_at_ns);
    // A no-open-order startup may legitimately recover nothing: NT only
    // populates the account/portfolio cache once its runner loop performs
    // startup reconciliation, and the live runtime feed re-seeds the
    // portfolio from on_account events after entry. Pre-existing open orders
    // are different: if they cannot be attributed to recovered reservation
    // metadata, submit admission would start with an unreconciled ledger and
    // could double-allocate capital, so fail closed before entering NT's
    // loop. This is a reconciliation guard, not the removed start gate.
    fail_closed_on_unreconciled_startup_rebuild(startup_rebuild)?;
    // Wire the durable kill-switch loss protection for the whole run: subscribe
    // the accumulator to position events and spawn its halt-action retry loop.
    // The guard unsubscribes and aborts the retry task on drop.
    let _loss_protection_guards = wire_bolt_v3_loss_protection_runtime(runtime);
    let node_handle = runtime.node.handle();
    let mut capture_guards = {
        let node = &runtime.node;
        wire_bolt_v3_runtime_capture(node, node_handle, loaded)
    }
    .map_err(BoltV3LiveNodeError::RuntimeCaptureWire)?;
    let mut capture_failure_receiver = capture_guards.take_failure_receiver();
    let iv_start_task = runtime.spawn_iv_engine_start_on_running(&loaded.root)?;

    let run_result = {
        let node = &mut runtime.node;
        let run_future = node.run();
        tokio::pin!(run_future);

        if let Some(receiver) = capture_failure_receiver.as_mut() {
            tokio::select! {
                result = &mut run_future => result,
                _ = receiver => {
                    log::error!("NT runtime capture failure detected, awaiting LiveNode shutdown");
                    run_future.await
                }
            }
        } else {
            run_future.await
        }
    };
    if let Some(task) = iv_start_task {
        task.abort();
    }
    let iv_stop_result = runtime.stop_iv_engine_lifecycle(&loaded.root);
    let shutdown_result = capture_guards.shutdown().await;

    let run_and_capture_result =
        classify_live_node_run_and_capture_shutdown(run_result, shutdown_result);
    match (run_and_capture_result, iv_stop_result) {
        (Err(run_or_capture_error), Err(iv_stop_error)) => {
            log::error!("IV lifecycle stop failed after live-node run failure: {iv_stop_error}");
            Err(run_or_capture_error)
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn fail_closed_on_unreconciled_startup_rebuild(
    startup_rebuild: BoltV3SubmitCapitalAdmissionRebuildDecision,
) -> Result<(), BoltV3LiveNodeError> {
    if !startup_rebuild.accepted && startup_rebuild.attempted_reservation_count > 0 {
        return Err(BoltV3LiveNodeError::StartupCapitalAdmissionRebuild(
            startup_rebuild,
        ));
    }
    Ok(())
}

fn strategy_free_start_timeout_secs(
    loaded: &LoadedBoltV3Config,
) -> Result<u64, BoltV3LiveNodeError> {
    loaded
        .root
        .nautilus
        .timeout_connection_secs
        .checked_add(loaded.root.nautilus.timeout_reconciliation_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_portfolio_secs))
        .ok_or(BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow)
}

fn strategy_free_stop_timeout_secs(
    loaded: &LoadedBoltV3Config,
) -> Result<u64, BoltV3LiveNodeError> {
    loaded
        .root
        .nautilus
        .timeout_disconnection_secs
        .checked_add(loaded.root.nautilus.delay_post_stop_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_shutdown_secs))
        .ok_or(BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow)
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
    // Enabled kill-switch boot must fail closed on an unresolved/corrupt/missing
    // durable record before constructing NT clients or registering
    // submit-capable strategy runtime. A clean recovery returns the latched
    // state to seed admission (before registration) and to sync NT trading
    // state (after build).
    let kill_switch_startup_state = recover_kill_switch_state_before_live_node_build(loaded)?;
    let loss_policy = loss_governor_policy_from_loaded(loaded)?;
    let loss_halt_action_policy = loss_governor_halt_action_policy_from_loaded(loaded)?;
    let capital_admission = capital_admission_config_from_loaded(loaded)?;
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
    let decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter> = if loaded.strategies.is_empty() {
        if loss_policy.is_none() && capital_admission.is_none() {
            Arc::new(NoStrategyDecisionEvidenceWriter)
        } else {
            Arc::new(
                JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(loaded).map_err(|error| {
                    BoltV3LiveNodeError::StrategyRegistration(
                        BoltV3StrategyRegistrationError::Evidence {
                            message: error.to_string(),
                        },
                    )
                })?,
            )
        }
    } else {
        Arc::new(
            JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(loaded).map_err(|error| {
                BoltV3LiveNodeError::StrategyRegistration(
                    BoltV3StrategyRegistrationError::Evidence {
                        message: error.to_string(),
                    },
                )
            })?,
        )
    };
    let startup_observed_at_ns = current_unix_nanos().map_err(BoltV3LiveNodeError::Build)?;
    let capital_admission_runtime_feed_config =
        capital_admission_runtime_feed_config_from_loaded(loaded, startup_observed_at_ns);
    let order_reject_observer_account_id = order_reject_observer_account_id_from_loaded(loaded);
    let capital_admission_venue_spendability_source =
        capital_admission_venue_spendability_source_config_from_loaded(loaded)?;
    let submit_reservation_recovery = if capital_admission_runtime_feed_config.is_some() {
        submit_reservation_recovery_config_from_loaded(loaded)?
    } else {
        None
    };
    let submit_admission = Arc::new(
        BoltV3SubmitAdmissionState::new_with_live_submit_limits_and_optional_controls(
            decision_evidence.clone(),
            live_submit_approval_limits,
            loss_policy.clone(),
            capital_admission,
        ),
    );
    // Latch the recovered kill-switch state into submit admission before any
    // submit-capable strategy runtime is registered, so a recovered halt blocks
    // submits from the first registered strategy onward.
    if let Some(state) = kill_switch_startup_state.as_ref() {
        submit_admission.replace_kill_switch_state(state.clone());
    }
    let (capital_admission_runtime_feed, capital_admission_runtime_feed_subscription) =
        match capital_admission_runtime_feed_config {
            Some(config) => {
                let feed = Arc::new(Mutex::new(CapitalAdmissionRuntimeFeed::new(
                    config,
                    submit_admission.clone(),
                )));
                let subscription = subscribe_capital_admission_runtime_feed(feed.clone());
                (Some(feed), Some(subscription))
            }
            None => (None, None),
        };
    let (order_reject_observer_feed, order_reject_observer_feed_subscription) =
        match order_reject_observer_account_id {
            Some(account_id) => {
                let feed = Arc::new(Mutex::new(BoltV3OrderRejectObserverFeed::new(
                    decision_evidence.clone(),
                    account_id,
                )));
                let subscription = subscribe_order_reject_observer_feed(feed.clone());
                (Some(feed), Some(subscription))
            }
            None => (None, None),
        };
    let order_execution_policy =
        crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::from_mode(
            loaded.root.runtime.order_execution_mode,
        );
    let strategy_execution_controls = BoltV3StrategyExecutionControls {
        submit_admission: submit_admission.clone(),
        order_execution_policy,
    };
    let builder =
        make_bolt_v3_live_node_builder(loaded).map_err(BoltV3LiveNodeError::BuilderConstruction)?;
    let (builder, summary) = register_bolt_v3_clients(builder, adapters)
        .map_err(BoltV3LiveNodeError::ClientRegistration)?;
    let mut node = builder.build().map_err(BoltV3LiveNodeError::Build)?;
    // Sync the recovered kill-switch state into NT's RiskEngine trading state so
    // the NT risk engine and the submit-admission latch agree on the halt. The
    // loss-protection seed below can override this for fail-closed cases.
    if let Some(state) = kill_switch_startup_state.as_ref() {
        sync_nt_trading_state_for_kill_switch(&mut node, state);
    }
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
            decision_evidence.clone(),
            iv_runtime,
        )
    } else {
        register_bolt_v3_strategies_on_node_with_bindings(
            &mut node,
            loaded,
            resolved,
            crate::strategy_bindings::production_runtime_bindings(),
            strategy_execution_controls,
            decision_evidence.clone(),
        )
    }
    .map_err(BoltV3LiveNodeError::StrategyRegistration)?;
    for strategy in &strategy_summary.registered {
        log::info!(
            "bolt-v3 registered strategy: strategy_instance_id={} strategy_archetype={} nt_strategy_id={}",
            strategy.strategy_instance_id,
            strategy.strategy_archetype.as_str(),
            strategy.registered_strategy_id
        );
    }
    // Configure the durable kill-switch loss-protection accumulator after
    // strategies are registered (its flatten targets are the registered NT
    // strategy ids) and seed it from the durable store. `seed_from_store` can
    // fail closed (e.g. an armed durable record with no loss snapshot becomes
    // `FailedManualIntervention`) and override the kill-switch state established
    // above by `recover_kill_switch_state_before_live_node_build`, so re-sync NT
    // trading state from the final loss-protection state — otherwise a
    // fail-closed seed would latch admission while leaving NT trading `Active`.
    let loss_protection =
        configure_bolt_v3_kill_switch_loss_protection(loaded, &node, submit_admission.clone())?;
    if let Some(protection) = loss_protection.as_ref() {
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
            capital_admission_runtime_feed_subscription,
            capital_admission_venue_spendability_source,
            submit_reservation_recovery,
        },
        iv_runtime,
        iv_event_bindings,
        resolved.redaction_values(),
    );
    runtime.refresh_capital_admission_venue_spendability_from_configured_source()?;
    Ok((runtime, summary))
}

fn loss_governor_runtime_feed_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<LossGovernorRuntimeFeedConfig>, BoltV3LiveNodeError> {
    let Some(block) = loaded.root.risk.loss_governor.as_ref() else {
        return Ok(None);
    };
    if !block.enabled {
        return Ok(None);
    }
    Ok(Some(LossGovernorRuntimeFeedConfig {
        account_id: block.account_id,
        rolling_window_ns: block.rolling_window_ns,
        active_position_pnl_max_entries: required_loss_governor_usize(
            "risk.loss_governor.active_position_pnl_max_entries",
            block.active_position_pnl_max_entries,
        )?,
    }))
}

fn capital_admission_runtime_feed_config_from_loaded(
    loaded: &LoadedBoltV3Config,
    startup_observed_at_ns: u64,
) -> Option<CapitalAdmissionRuntimeFeedConfig> {
    let pools = loaded.root.risk.capital_pools.as_ref()?;
    let pool = pools.iter().find(|pool| pool.enforce_submit_admission)?;
    let product = pool.prediction_market_binary.as_ref()?;
    Some(CapitalAdmissionRuntimeFeedConfig {
        venue_id: pool.venue_id.clone(),
        account_id: pool.account_id,
        collateral_currency: pool.collateral_currency.clone(),
        product_state: ProductAdmissionSnapshot::PredictionMarketBinary(
            PredictionMarketAdmissionSnapshot {
                source: "bolt_configured_binary_product".to_string(),
                observed_at_ns: startup_observed_at_ns,
                yes_instrument_id: product.yes_instrument_id.to_string(),
                no_instrument_id: product.no_instrument_id.to_string(),
                yes_position: Decimal::ZERO,
                no_position: Decimal::ZERO,
                collateral_allowance: Decimal::ZERO,
                conditional_token_allowance: Decimal::ZERO,
                collateral_coupled_group_id: product.collateral_coupled_group_id.clone(),
            },
        ),
        startup_observed_at_ns,
        dedupe_retention_ns: pool.dedupe_retention_ns,
    })
}

fn order_reject_observer_account_id_from_loaded(loaded: &LoadedBoltV3Config) -> Option<AccountId> {
    let pools = loaded.root.risk.capital_pools.as_ref()?;
    let pool = pools.iter().find(|pool| pool.enforce_submit_admission)?;
    Some(pool.account_id)
}

fn capital_admission_venue_spendability_source_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<BoltV3CapitalAdmissionVenueSpendabilitySourceConfig>, BoltV3LiveNodeError> {
    let Some(pool) = loaded
        .root
        .risk
        .capital_pools
        .as_ref()
        .and_then(|pools| pools.iter().find(|pool| pool.enforce_submit_admission))
    else {
        return Ok(None);
    };
    let has_source_binding = pool.venue_spendability_source_path.is_some()
        || pool.venue_spendability_source_sha256.is_some()
        || pool.venue_spendability_source_max_bytes.is_some();
    if !has_source_binding {
        return Ok(None);
    }
    let (Some(path_value), Some(expected_sha256), Some(max_bytes)) = (
        pool.venue_spendability_source_path.as_ref(),
        pool.venue_spendability_source_sha256.as_ref(),
        pool.venue_spendability_source_max_bytes,
    ) else {
        return Err(BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "risk.capital_pools venue_spendability_source path, sha256, and max_bytes must be configured together"
        )));
    };
    Ok(Some(BoltV3CapitalAdmissionVenueSpendabilitySourceConfig {
        path: resolve_root_relative_path(&loaded.root_path, path_value),
        max_bytes,
        expected_sha256: expected_sha256.clone(),
        venue_id: pool.venue_id.clone(),
        account_id: pool.account_id.to_string(),
        collateral_currency: pool.collateral_currency.clone(),
    }))
}

/// Resolve the startup reservation-recovery source from the loaded config.
/// The recovery driver reads the decision-evidence file, so the path comes
/// from [`decision_evidence_path`] and the read bound from
/// `persistence.decision_evidence.recovery_evidence_max_bytes`. Returns
/// `None` (recovery disabled) when the byte cap is not configured.
fn submit_reservation_recovery_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<BoltV3SubmitReservationRecoveryConfig>, BoltV3LiveNodeError> {
    let Some(max_bytes) = loaded
        .root
        .persistence
        .decision_evidence
        .recovery_evidence_max_bytes
    else {
        return Ok(None);
    };
    Ok(Some(BoltV3SubmitReservationRecoveryConfig {
        path: decision_evidence_path(loaded).map_err(BoltV3LiveNodeError::Build)?,
        max_bytes,
    }))
}

fn capital_admission_venue_spendability_snapshot_from_source_config(
    config: &BoltV3CapitalAdmissionVenueSpendabilitySourceConfig,
) -> Result<VenueSpendabilitySnapshot, BoltV3LiveNodeError> {
    venue_spendability_snapshot_from_json_file(VenueSpendabilitySourceFileRequest {
        path: &config.path,
        max_bytes: config.max_bytes,
        expected_sha256: &config.expected_sha256,
        identity: VenueSpendabilityIdentity {
            venue_id: &config.venue_id,
            account_id: &config.account_id,
            collateral_currency: &config.collateral_currency,
        },
    })
    .map_err(|error| {
        BoltV3LiveNodeError::Build(anyhow::anyhow!(
            "capital admission venue spendability source rejected: {error:?}"
        ))
    })
}

fn refresh_capital_admission_venue_spendability_from_source(
    feed: &Arc<Mutex<CapitalAdmissionRuntimeFeed>>,
    config: &BoltV3CapitalAdmissionVenueSpendabilitySourceConfig,
) -> Result<Option<BoltV3SubmitCapitalAdmissionNtComponents>, BoltV3LiveNodeError> {
    let snapshot = capital_admission_venue_spendability_snapshot_from_source_config(config)?;
    let mut feed = feed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(feed.on_venue_spendability_snapshot(snapshot))
}

fn capital_admission_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<BoltV3SubmitCapitalAdmissionConfig>, BoltV3LiveNodeError> {
    let Some(pools) = loaded.root.risk.capital_pools.as_ref() else {
        return Ok(None);
    };
    let Some(pool) = pools.iter().find(|pool| pool.enforce_submit_admission) else {
        return Ok(None);
    };
    Ok(Some(BoltV3SubmitCapitalAdmissionConfig {
        venue_id: pool.venue_id.clone(),
        account_id: pool.account_id.to_string(),
        product_kind: ProductKind::PredictionMarketBinary,
        collateral_currency: pool.collateral_currency.clone(),
        capital_pool: CapitalPoolSnapshot {
            source: pool.pool_id.clone(),
            observed_at_ns: 0,
            pool_id: pool.pool_id.clone(),
            max_pool_liability: required_pool_decimal(
                "risk.capital_pools.max_pool_liability",
                &pool.max_pool_liability,
            )?,
            committed_liability: Decimal::ZERO,
            max_snapshot_age_ns: pool.max_snapshot_age_ns,
        },
        policy: capital_admission_policy_from_pool(pool)?,
        dedupe_retention_ns: pool.dedupe_retention_ns,
    }))
}

fn capital_admission_policy_from_pool(
    pool: &CapitalPoolBlock,
) -> Result<CapitalAdmissionPolicy, BoltV3LiveNodeError> {
    let policy = &pool.capital_admission_policy;
    Ok(CapitalAdmissionPolicy {
        min_remaining_pool_balance: optional_pool_decimal(
            "risk.capital_pools.capital_admission_policy.min_remaining_pool_balance",
            policy.min_remaining_pool_balance.as_deref(),
        )?,
        fee_slippage_policy: Some(FeeSlippagePolicy {
            max_fee_liability: required_pool_decimal(
                "risk.capital_pools.capital_admission_policy.fee_slippage.max_fee_liability",
                &policy.fee_slippage.max_fee_liability,
            )?,
            max_slippage_liability: required_pool_decimal(
                "risk.capital_pools.capital_admission_policy.fee_slippage.max_slippage_liability",
                &policy.fee_slippage.max_slippage_liability,
            )?,
        }),
    })
}

fn required_pool_decimal(label: &str, value: &str) -> Result<Decimal, BoltV3LiveNodeError> {
    parse_decimal_string(value).map_err(|message| {
        BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be a decimal string: {message}"
        ))
    })
}

fn optional_pool_decimal(
    label: &str,
    value: Option<&str>,
) -> Result<Option<Decimal>, BoltV3LiveNodeError> {
    value
        .map(|value| required_pool_decimal(label, value))
        .transpose()
}

fn loss_governor_policy_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<LossGovernorPolicy>, BoltV3LiveNodeError> {
    let Some(block) = loaded.root.risk.loss_governor.as_ref() else {
        return Ok(None);
    };
    if !block.enabled {
        return Ok(None);
    }
    Ok(Some(LossGovernorPolicy {
        max_snapshot_age_ns: block.max_snapshot_age_ns,
        max_per_trade_loss: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_per_trade_loss",
            block.max_per_trade_loss.as_deref(),
        )?),
        max_daily_loss: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_daily_loss",
            block.max_daily_loss.as_deref(),
        )?),
        max_rolling_loss: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_rolling_loss",
            block.max_rolling_loss.as_deref(),
        )?),
        max_drawdown: Some(required_loss_governor_decimal(
            "risk.loss_governor.max_drawdown",
            block.max_drawdown.as_deref(),
        )?),
    }))
}

fn loss_governor_halt_action_policy_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<LossGovernorHaltActionPolicy>, BoltV3LiveNodeError> {
    let Some(block) = loaded.root.risk.loss_governor.as_ref() else {
        return Ok(None);
    };
    if !block.enabled {
        return Ok(None);
    }
    Ok(Some(LossGovernorHaltActionPolicy {
        on_loss_breach_trading_state: required_loss_governor_trading_state_action(
            "risk.loss_governor.on_loss_breach_trading_state",
            block.on_loss_breach_trading_state,
        )?,
        on_untrusted_snapshot_trading_state: required_loss_governor_trading_state_action(
            "risk.loss_governor.on_untrusted_snapshot_trading_state",
            block.on_untrusted_snapshot_trading_state,
        )?,
        recovery_mode: required_loss_governor_recovery_mode(
            "risk.loss_governor.recovery_mode",
            block.recovery_mode,
        )?,
        manual_recovery_evidence_max_path_bytes: required_loss_governor_usize(
            "risk.loss_governor.manual_recovery_evidence_max_path_bytes",
            block.manual_recovery_evidence_max_path_bytes,
        )?,
    }))
}

fn required_loss_governor_trading_state_action(
    label: &'static str,
    value: Option<LossGovernorTradingStateAction>,
) -> Result<LossGovernorTradingStateAction, BoltV3LiveNodeError> {
    value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))
}

fn required_loss_governor_recovery_mode(
    label: &'static str,
    value: Option<LossGovernorRecoveryMode>,
) -> Result<LossGovernorRecoveryMode, BoltV3LiveNodeError> {
    value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))
}

fn required_loss_governor_usize(
    label: &'static str,
    value: Option<usize>,
) -> Result<usize, BoltV3LiveNodeError> {
    let value =
        value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))?;
    if value == usize::MIN {
        return Err(BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be positive"
        )));
    }
    Ok(value)
}

/// Reads the durable kill-switch state before the live node is built.
///
/// Enabled kill-switch boot must fail closed before resolving secrets,
/// constructing NT clients, or registering submit-capable strategy runtime when
/// the durable store holds an unresolved/corrupt/missing record. A disabled (or
/// unconfigured) kill switch carries no durable state requirement.
fn recover_kill_switch_state_before_live_node_build(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<KillSwitchState>, BoltV3LiveNodeError> {
    let Some(config) = loaded.root.risk.kill_switch.as_ref() else {
        return Ok(None);
    };
    if !config.enabled {
        return Ok(None);
    }

    let store = KillSwitchStore::from_root_config_path(&loaded.root_path, config);
    match store
        .load_recovery_state()
        .map_err(BoltV3LiveNodeError::KillSwitchStore)?
    {
        KillSwitchRecoveryState::Recovered(state) => Ok(Some(state)),
        KillSwitchRecoveryState::FailClosed { reason, .. } => {
            Err(BoltV3LiveNodeError::KillSwitchRecovery { reason })
        }
    }
}

/// Syncs a recovered/seeded kill-switch state into NT's `RiskEngine` trading
/// state so the NT risk engine and the submit-admission latch agree on the halt
/// after a restart, instead of leaving NT trading `Active` behind a latched
/// admission.
fn sync_nt_trading_state_for_kill_switch(node: &mut LiveNode, state: &KillSwitchState) {
    let Some(trading_state) = nt_trading_state_for_kill_switch_state(state) else {
        return;
    };
    node.kernel()
        .risk_engine()
        .borrow_mut()
        .set_trading_state(trading_state);
}

fn nt_trading_state_for_kill_switch_state(state: &KillSwitchState) -> Option<TradingState> {
    match state {
        KillSwitchState::Armed => None,
        KillSwitchState::Halting { .. }
        | KillSwitchState::Halted { .. }
        | KillSwitchState::Cancelling { .. }
        | KillSwitchState::Flattening { .. } => Some(TradingState::Reducing),
        KillSwitchState::Flat { .. } | KillSwitchState::FailedManualIntervention { .. } => {
            Some(TradingState::Halted)
        }
    }
}

/// Moves the NT risk engine to `Reducing` after a loss-halt action. Abstracted
/// as a trait so the hard kill-switch sink is unit-testable without a live NT
/// risk engine. The transition is venue-neutral: it applies regardless of the
/// global execution mode and does not submit, cancel, or close venue orders.
trait TradingStateController {
    fn enter_reducing(&self);
}

/// Closure-backed `TradingStateController`. The production caller captures the
/// NT risk-engine handle in the closure so the concrete NT risk-engine type
/// does not have to be named here; tests inject a recording controller instead.
struct ClosureTradingStateController<F: Fn()> {
    enter_reducing: F,
}

impl<F: Fn()> TradingStateController for ClosureTradingStateController<F> {
    fn enter_reducing(&self) {
        (self.enter_reducing)();
    }
}

/// Hard kill-switch loss-action sink that drives the NT runtime on a durable
/// daily-realized loss breach.
///
/// On a fresh `FlattenPositions` halt it moves the NT risk engine to
/// `Reducing` (venue-neutral). Active market exit is intentionally not wired:
/// current source-fence policy forbids NT `ExitMarket`/market-exit control
/// paths because they bypass Bolt's submit/cancel chokepoints. Config
/// validation rejects `flatten_open_positions_on_breach = true` until a shared
/// execution-policy flatten path exists.
struct NtReducingLossActionSink {
    trading_state: Rc<dyn TradingStateController>,
    dispatched_halts: RefCell<BTreeSet<String>>,
}

impl NtReducingLossActionSink {
    fn new(trading_state: Rc<dyn TradingStateController>) -> Self {
        Self {
            trading_state,
            dispatched_halts: RefCell::new(BTreeSet::new()),
        }
    }
}

impl KillSwitchLossActionSink for NtReducingLossActionSink {
    fn emit(&self, action: KillSwitchLossAction) -> Result<()> {
        if action.kind != KillSwitchLossActionKind::FlattenPositions {
            return Ok(());
        }
        if self.dispatched_halts.borrow().contains(&action.halt_id) {
            return Ok(());
        }
        // `Reducing` is the whole live action in this slice. Venue-mutating
        // market exit must stay out until it can be routed through a shared
        // execution-policy boundary.
        self.trading_state.enter_reducing();
        self.dispatched_halts
            .borrow_mut()
            .insert(action.halt_id.clone());
        Ok(())
    }
}

/// Configures the durable kill-switch loss-protection accumulator from the
/// validated `risk.kill_switch` block.
///
/// Returns `Ok(None)` when the kill switch is absent or disabled. Otherwise it
/// builds the daily-realized accumulator, wires its live action to the NT
/// `Reducing` trading-state transition, then seeds it from the durable store. A
/// failed seed can fail closed to a halted state; the caller syncs NT trading
/// state from the resulting state.
fn configure_bolt_v3_kill_switch_loss_protection(
    loaded: &LoadedBoltV3Config,
    node: &LiveNode,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
) -> Result<Option<Rc<RefCell<KillSwitchLossProtection>>>, BoltV3LiveNodeError> {
    let Some(kill_switch) = loaded
        .root
        .risk
        .kill_switch
        .as_ref()
        .filter(|kill_switch| kill_switch.enabled)
    else {
        return Ok(None);
    };
    if kill_switch.flatten_open_positions_on_breach {
        return Err(BoltV3LiveNodeError::KillSwitchLossProtection(
            anyhow::anyhow!(
                "risk.kill_switch.flatten_open_positions_on_breach=true is not supported until a shared execution-policy flatten path exists"
            ),
        ));
    }
    let max_utc_daily_realized_loss =
        parse_decimal_string(&kill_switch.max_utc_daily_realized_loss).map_err(|reason| {
            BoltV3LiveNodeError::KillSwitchLossProtection(anyhow::anyhow!(
                "risk.kill_switch.max_utc_daily_realized_loss parse failed: {reason}"
            ))
        })?;
    let config = KillSwitchLossProtectionConfig {
        max_utc_daily_realized_loss,
        action_retry_interval_ms: kill_switch.action_retry_interval_ms,
        action_retry_timeout_ms: kill_switch.action_retry_timeout_ms,
        account_ids: kill_switch.account_ids.clone(),
        instrument_ids: kill_switch.instrument_ids.clone(),
    };
    let store = KillSwitchStore::from_root_config_path(&loaded.root_path, kill_switch);
    let risk_engine = node.kernel().risk_engine().clone();
    let trading_state: Rc<dyn TradingStateController> = Rc::new(ClosureTradingStateController {
        enter_reducing: move || {
            risk_engine
                .borrow_mut()
                .set_trading_state(TradingState::Reducing);
        },
    });
    let action_sink = Rc::new(NtReducingLossActionSink::new(trading_state));
    let mut protection =
        KillSwitchLossProtection::new(config, submit_admission, store, action_sink)
            .map_err(BoltV3LiveNodeError::KillSwitchLossProtection)?;
    let recovery_action_clock_unix_nanos =
        current_unix_nanos().map_err(BoltV3LiveNodeError::KillSwitchLossProtection)?;
    protection
        .seed_from_store(recovery_action_clock_unix_nanos)
        .map_err(BoltV3LiveNodeError::KillSwitchLossProtection)?;
    Ok(Some(Rc::new(RefCell::new(protection))))
}

/// Runtime guards for the durable kill-switch loss protection. Owns the
/// position-event subscription and the action-retry task; dropping it
/// unsubscribes and aborts the retry loop.
struct BoltV3LossProtectionRuntimeGuards {
    position_events: Option<TypedHandler<PositionEvent>>,
    retry_handle: Option<tokio::task::JoinHandle<()>>,
}

impl BoltV3LossProtectionRuntimeGuards {
    fn none() -> Self {
        Self {
            position_events: None,
            retry_handle: None,
        }
    }
}

impl Drop for BoltV3LossProtectionRuntimeGuards {
    fn drop(&mut self) {
        if let Some(position_events) = self.position_events.take() {
            unsubscribe_position_events(position_events_pattern(), &position_events);
        }
        if let Some(retry_handle) = self.retry_handle.take() {
            retry_handle.abort();
        }
    }
}

/// Wires the durable kill-switch loss protection into NT's runtime: subscribes
/// the accumulator to position events (so realized PnL drives the daily breach)
/// and spawns the pending-halt-action retry loop. A no-op when no kill switch
/// is configured.
fn wire_bolt_v3_loss_protection_runtime(
    runtime: &BoltV3LiveNodeRuntime,
) -> BoltV3LossProtectionRuntimeGuards {
    let Some(loss_protection) = runtime.loss_protection.as_ref() else {
        return BoltV3LossProtectionRuntimeGuards::none();
    };
    let retry_interval_ms = loss_protection.borrow().action_retry_interval_ms();
    let retry_loss_protection = Rc::clone(loss_protection);
    let retry_handle = tokio::task::spawn_local(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(retry_interval_ms));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let now_unix_nanos = match current_unix_nanos() {
                Ok(now_unix_nanos) => now_unix_nanos,
                Err(error) => {
                    log::error!("bolt-v3 kill-switch loss protection retry clock failed: {error}");
                    continue;
                }
            };
            if let Err(error) = retry_loss_protection
                .borrow_mut()
                .poll_pending_halt_actions(now_unix_nanos)
            {
                log::error!("bolt-v3 kill-switch loss protection action retry failed: {error}");
            }
        }
    });
    let loss_protection = Rc::clone(loss_protection);
    let position_events = TypedHandler::from(move |event: &PositionEvent| {
        if let Err(error) = loss_protection.borrow_mut().record_position_event(event) {
            log::error!(
                "bolt-v3 kill-switch loss protection position-event handling failed: {error}"
            );
        }
    });
    subscribe_position_events(position_events_pattern(), position_events.clone(), None);
    BoltV3LossProtectionRuntimeGuards {
        position_events: Some(position_events),
        retry_handle: Some(retry_handle),
    }
}

fn loss_governor_halt_action_handler_from_node(
    node: &LiveNode,
    loss_policy: LossGovernorPolicy,
    action_policy: LossGovernorHaltActionPolicy,
) -> LossGovernorHaltActionHandler {
    // The handler only needs to read and flip NT's `RiskEngine` trading state; the
    // node is solely the source of that engine handle. Delegate to the node-free
    // core via trading-state accessors so the halt behaviour can be exercised
    // without building a `NautilusKernel`, whose logging init claims the
    // process-global `log` slot and is mutually exclusive with a test's
    // capturing logger.
    let read_engine = node.kernel().risk_engine().clone();
    let write_engine = read_engine.clone();
    loss_governor_halt_action_handler(
        Rc::new(move || read_engine.borrow().trading_state()),
        Rc::new(move |state| write_engine.borrow_mut().set_trading_state(state)),
        loss_policy,
        action_policy,
    )
}

/// Node-free core of the loss-governor halt action handler. Reads and flips the
/// trading state via the supplied accessors rather than a `LiveNode`/`RiskEngine`
/// handle, so the halt logic can be exercised against plain state cells without
/// building a `NautilusKernel` (whose logging init owns the global `log` slot).
fn loss_governor_halt_action_handler(
    read_trading_state: Rc<dyn Fn() -> TradingState>,
    set_trading_state: Rc<dyn Fn(TradingState)>,
    loss_policy: LossGovernorPolicy,
    action_policy: LossGovernorHaltActionPolicy,
) -> LossGovernorHaltActionHandler {
    Rc::new(move |snapshot, now_ns, source_observations| {
        let decision = evaluate_loss_admission_with_observations(
            &loss_policy,
            snapshot,
            now_ns,
            source_observations,
        );
        let current_state = read_trading_state();
        if current_state == TradingState::Active && decision.accepted {
            return;
        }

        if let Some(target_state) =
            next_loss_governor_trading_state(&action_policy, current_state, &decision)
        {
            // Apply the trading-state transition unconditionally — the
            // loss-governor halt itself MUST always fire.
            set_trading_state(target_state);
        }
    })
}

fn required_loss_governor_decimal(
    label: &'static str,
    value: Option<&str>,
) -> Result<Decimal, BoltV3LiveNodeError> {
    let value =
        value.ok_or_else(|| BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!("{label} missing")))?;
    let decimal = parse_decimal_string(value).map_err(|reason| {
        BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be a valid decimal string ({reason}): `{value}`"
        ))
    })?;
    if decimal <= Decimal::ZERO {
        return Err(BoltV3LiveNodeError::RiskPolicy(anyhow::anyhow!(
            "{label} must be positive: `{value}`"
        )));
    }
    Ok(decimal)
}

#[cfg(test)]
mod tests;
