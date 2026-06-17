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
    msgbus::{self, MStr, Pattern, ShareableMessageHandler, TypedHandler, switchboard},
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
    identifiers::{ClientId, InstrumentId, OptionSeriesId, StrategyId, Venue},
    instruments::{Instrument, InstrumentAny},
    types::Price,
};
#[cfg(test)]
use nautilus_model::{enums::AggressorSide, identifiers::TradeId};
use nautilus_model::{
    enums::{OrderSide, OrderType, TradingState},
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
        BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence,
        BoltV3PositionSizerRebuildAuditEvidence, BoltV3StrategyInputEvidenceSnapshot,
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
    bolt_v3_loss_governor::{LossGovernorPolicy, evaluate_loss_admission},
    bolt_v3_loss_halt_actions::{
        LossGovernorHaltActionHandler, LossGovernorHaltActionPolicy,
        LossGovernorManualRecoveryEvidence, LossGovernorManualRecoveryRequest,
        LossGovernorRecoveryMode, LossGovernorTradingStateAction,
        next_loss_governor_manual_recovery_trading_state, next_loss_governor_trading_state,
    },
    bolt_v3_loss_runtime_feed::{
        LossGovernorRuntimeFeed, LossGovernorRuntimeFeedConfig,
        LossGovernorRuntimeFeedSubscription, subscribe_loss_governor_runtime_feed,
    },
    bolt_v3_position_sizer::{
        FeeSlippagePolicy, PredictionMarketSizingSnapshot, ProductKind, ProductSizingSnapshot,
        SizingPolicy,
    },
    bolt_v3_position_sizer_runtime_feed::{
        PositionSizerRuntimeFeed, PositionSizerRuntimeFeedConfig,
        PositionSizerRuntimeFeedSubscription, subscribe_position_sizer_runtime_feed,
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
    bolt_v3_sizing_state::{
        VenueSpendabilityIdentity, VenueSpendabilitySnapshot, VenueSpendabilitySourceFileRequest,
        venue_spendability_snapshot_from_json_file,
    },
    bolt_v3_strategy_registration::{
        BoltV3StrategyExecutionControls, BoltV3StrategyRegistrationError,
        register_bolt_v3_strategies_on_node_with_bindings,
        register_bolt_v3_strategies_on_node_with_iv_runtime_bindings,
    },
    bolt_v3_submit_admission::{
        BoltV3CompiledOrderSide, BoltV3LiveSubmitApprovalLimits, BoltV3SubmitAdmissionState,
        BoltV3SubmitPositionSizerConfig, BoltV3SubmitPositionSizingMissingNtAccountCacheBalance,
        BoltV3SubmitPositionSizingNtComponents, BoltV3SubmitPositionSizingOpenOrderEvidence,
        BoltV3SubmitPositionSizingOpenOrderSnapshot, BoltV3SubmitPositionSizingRebuildDecision,
    },
    bolt_v3_validate::parse_decimal_string,
    nt_runtime_capture::{NtRuntimeCaptureGuards, wire_nt_runtime_capture},
    secrets::SsmResolverSession,
};

pub fn current_build_head_sha() -> Option<&'static str> {
    option_env!("BOLT_V3_BUILD_HEAD_SHA").filter(|value| is_git_head_sha(value))
}

fn is_git_head_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub struct BoltV3LiveNodeRuntime {
    node: LiveNode,
    registration_summary: BoltV3RegistrationSummary,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    loss_halt_action_policy: Option<LossGovernorHaltActionPolicy>,
    loss_runtime_feed: Option<Rc<RefCell<LossGovernorRuntimeFeed>>>,
    loss_runtime_feed_subscription: Option<LossGovernorRuntimeFeedSubscription>,
    position_sizer_runtime_feed: Option<Arc<Mutex<PositionSizerRuntimeFeed>>>,
    position_sizer_runtime_feed_subscription: Option<PositionSizerRuntimeFeedSubscription>,
    position_sizer_venue_spendability_source:
        Option<BoltV3PositionSizerVenueSpendabilitySourceConfig>,
    submit_reservation_recovery: Option<BoltV3SubmitReservationRecoveryConfig>,
    iv_runtime: Option<IvRuntimeEngine>,
    iv_event_bindings: Option<BoltV3IvRuntimeEventBindings>,
    redaction_values: Vec<Zeroizing<String>>,
}

#[derive(Debug, Clone)]
struct BoltV3PositionSizerVenueSpendabilitySourceConfig {
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

    fn record_position_sizer_rebuild_audit(
        &self,
        _audit: &BoltV3PositionSizerRebuildAuditEvidence,
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
}

struct BoltV3LiveNodeRuntimeFeeds {
    loss_halt_action_policy: Option<LossGovernorHaltActionPolicy>,
    loss_runtime_feed: Option<Rc<RefCell<LossGovernorRuntimeFeed>>>,
    loss_runtime_feed_subscription: Option<LossGovernorRuntimeFeedSubscription>,
    position_sizer_runtime_feed: Option<Arc<Mutex<PositionSizerRuntimeFeed>>>,
    position_sizer_runtime_feed_subscription: Option<PositionSizerRuntimeFeedSubscription>,
    position_sizer_venue_spendability_source:
        Option<BoltV3PositionSizerVenueSpendabilitySourceConfig>,
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
            loss_halt_action_policy: feeds.loss_halt_action_policy,
            loss_runtime_feed: feeds.loss_runtime_feed,
            loss_runtime_feed_subscription: feeds.loss_runtime_feed_subscription,
            position_sizer_runtime_feed: feeds.position_sizer_runtime_feed,
            position_sizer_runtime_feed_subscription: feeds
                .position_sizer_runtime_feed_subscription,
            position_sizer_venue_spendability_source: feeds
                .position_sizer_venue_spendability_source,
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

    pub fn nt_risk_trading_state(&self) -> TradingState {
        self.node.kernel().risk_engine().borrow().trading_state()
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

    pub fn position_sizer_configured(&self) -> bool {
        self.submit_admission.position_sizer_configured()
    }

    pub fn position_sizer_runtime_feed_configured(&self) -> bool {
        self.position_sizer_runtime_feed.is_some()
            && self.position_sizer_runtime_feed_subscription.is_some()
    }

    pub fn refresh_position_sizer_venue_spendability_from_configured_source(
        &self,
    ) -> Result<Option<BoltV3SubmitPositionSizingNtComponents>, BoltV3LiveNodeError> {
        let Some(config) = self.position_sizer_venue_spendability_source.as_ref() else {
            return Ok(None);
        };
        let Some(feed) = self.position_sizer_runtime_feed.as_ref() else {
            return Err(BoltV3LiveNodeError::Build(anyhow::anyhow!(
                "position sizer venue spendability source configured without runtime feed"
            )));
        };
        refresh_position_sizer_venue_spendability_from_source(feed, config)
    }

    pub fn position_sizer_reconciled(&self) -> Option<bool> {
        self.submit_admission.position_sizer_reconciled()
    }

    /// Rebuild the position sizer's capital-reservation ledger from the
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
    pub fn rebuild_position_sizer_from_nt_cache(
        &self,
        now_ns: u64,
    ) -> BoltV3SubmitPositionSizingRebuildDecision {
        let (account_id, binary_instrument_ids, collateral_currency) =
            match self.position_sizer_runtime_feed.as_ref() {
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
                let missing = BoltV3SubmitPositionSizingMissingNtAccountCacheBalance {
                    account_id: account_id.to_string(),
                    collateral_currency: collateral_currency.to_string(),
                };
                log::warn!(
                    "bolt-v3 position sizer startup rebuild could not seed account portfolio snapshot because NT cache is missing account_id={} collateral_currency={}",
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
        if let Some(feed) = self.position_sizer_runtime_feed.as_ref() {
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
                .position_sizing_open_order_reservation_from_known_metadata(evidence, recovered)
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
            .rebuild_position_sizing_open_order_snapshot(
                BoltV3SubmitPositionSizingOpenOrderSnapshot {
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
) -> Option<BoltV3SubmitPositionSizingOpenOrderEvidence> {
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
    Some(BoltV3SubmitPositionSizingOpenOrderEvidence {
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
    /// The startup position-sizer rebuild from the NT cache could not
    /// attribute one or more pre-existing open orders to recovered
    /// submit-reservation metadata, so submit admission would arm with an
    /// unreconciled capital-reservation ledger. The live runner refuses to
    /// enter NT's loop in this state to avoid double-allocating capital
    /// against orders it cannot account for. The wrapped decision carries
    /// the attempted/rebuilt reservation counts and rejection reason
    /// captured at boot. This is intentionally outside the removed start
    /// gate: it is a fail-closed reconciliation guard, not the live-canary
    /// arm gate, so it never reintroduces a gate-report/arm sequence.
    StartupPositionSizerRebuild(BoltV3SubmitPositionSizingRebuildDecision),
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
            BoltV3LiveNodeError::StartupPositionSizerRebuild(decision) => write!(
                f,
                "bolt-v3 startup position-sizer rebuild rejected runtime start: {decision:?}"
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
            | BoltV3LiveNodeError::StrategyFreeStartTimeout { .. }
            | BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow
            | BoltV3LiveNodeError::StrategyFreeStartIncomplete
            | BoltV3LiveNodeError::StrategyFreeExecutionAccountsMissing { .. }
            | BoltV3LiveNodeError::StrategyFreeReferenceProbeFailed { .. }
            | BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed { .. }
            | BoltV3LiveNodeError::StrategyFreeStopTimeout { .. }
            | BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow
            | BoltV3LiveNodeError::StartupPositionSizerRebuild(..) => None,
            BoltV3LiveNodeError::DisconnectFailed(error)
            | BoltV3LiveNodeError::StrategyFreeReferenceProbeSetup(error)
            | BoltV3LiveNodeError::StrategyFreeStartFailed(error)
            | BoltV3LiveNodeError::StrategyFreeStopFailed(error) => Some(error.as_ref()),
        }
    }
}

pub fn build_bolt_v3_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    // RV source-client validation is owned by the strategy-registration
    // chokepoint; trade transport must retain the clients it will validate.
    let transport_loaded =
        trade_transport_loaded_config(loaded, RealizedVolatilityTransportScope::Subscribed)?;
    let resolved = resolve_bolt_v3_live_node_secrets(&transport_loaded)?;
    let bundle =
        live_node_adapter_bundle_with_provider_live_submit_approvals(&transport_loaded, &resolved)?;
    let (runtime, _summary) = build_live_node_with_clients_and_submit_approval_limits(
        &transport_loaded,
        &resolved,
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
    let adapters = strategy_free_transport_adapter_configs(&transport_loaded, &resolved)?;
    let strategy_free_loaded = strategy_free_transport_loaded_config(&transport_loaded);
    let (runtime, _summary) =
        build_live_node_with_clients(&strategy_free_loaded, &resolved, adapters)?;
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

/// Prunes the loaded config to the client set needed by the trade transport.
///
/// RV source-client validation is intentionally enforced at the strategy
/// registration chokepoint, where the shared RV runtime is about to subscribe.
/// Callers that will register strategies must pass
/// [`RealizedVolatilityTransportScope::Subscribed`] so that chokepoint validates
/// clients present on the node transport, not merely clients present in TOML.
fn trade_transport_loaded_config(
    loaded: &LoadedBoltV3Config,
    rv_scope: RealizedVolatilityTransportScope,
) -> Result<LoadedBoltV3Config, BoltV3LiveNodeError> {
    let required_clients = trade_transport_client_keys(loaded, rv_scope)?;
    if required_clients.is_empty() {
        let mut transport_loaded = loaded.clone();
        transport_loaded.root.clients.clear();
        return Ok(transport_loaded);
    }
    let missing_clients = required_clients
        .iter()
        .filter(|client_key| !loaded.root.clients.contains_key(*client_key))
        .cloned()
        .collect::<Vec<_>>();
    if !missing_clients.is_empty() {
        return Err(BoltV3LiveNodeError::LiveTransportScope {
            reason: format!(
                "strategy references unconfigured client(s): {}",
                missing_clients.join(", ")
            ),
        });
    }

    let mut transport_loaded = loaded.clone();
    transport_loaded
        .root
        .clients
        .retain(|client_key, _| required_clients.contains(client_key));
    validate_trade_transport_execution_venue_cardinality(&transport_loaded)?;
    Ok(transport_loaded)
}

fn validate_trade_transport_execution_venue_cardinality(
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let mut execution_clients_by_venue: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (client_key, client) in &loaded.root.clients {
        if client.execution.is_some() {
            if let Some(client_keys) = execution_clients_by_venue.get_mut(client.venue.as_str()) {
                client_keys.push(client_key.clone());
            } else {
                execution_clients_by_venue
                    .insert(client.venue.as_str().to_string(), vec![client_key.clone()]);
            }
        }
    }
    for (venue, client_keys) in execution_clients_by_venue {
        if client_keys.len() > 1 {
            return Err(BoltV3LiveNodeError::LiveTransportScope {
                reason: format!(
                    "multiple execution clients share venue `{}` in the live transport scope: {}; only one execution client may be active per venue",
                    venue,
                    client_keys.join(", ")
                ),
            });
        }
    }
    Ok(())
}

/// Whether this transport build path will register strategies whose shared RV
/// runtime subscribes configured realized-volatility source data.
///
/// Trade builders use [`Subscribed`](Self::Subscribed). Strategy-free health and
/// probe builders use [`NotSubscribed`](Self::NotSubscribed): they may receive a
/// loaded config with strategies for adapter planning, but they clear strategy
/// actors before runtime and never subscribe RV. A future strategy-free path
/// that does subscribe RV must explicitly choose `Subscribed`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RealizedVolatilityTransportScope {
    Subscribed,
    NotSubscribed,
}

fn trade_transport_client_keys(
    loaded: &LoadedBoltV3Config,
    rv_scope: RealizedVolatilityTransportScope,
) -> Result<BTreeSet<String>, BoltV3LiveNodeError> {
    let mut client_keys = BTreeSet::new();
    for strategy in &loaded.strategies {
        client_keys.insert(strategy.config.execution_client_id.to_string());
        if let Some(reference_current_price) = strategy.config.reference_current_price.as_ref() {
            client_keys.extend(
                reference_current_price
                    .sources
                    .values()
                    .filter(|source| {
                        reference_price_source_is_runtime_available(reference_current_price, source)
                    })
                    .map(|source| source.client_id.to_string()),
            );
        }
        for signal in strategy.config.signal_data.values() {
            client_keys.insert(signal.data_client_id.to_string());
        }
        if let Some(resolution) = strategy.config.resolution_data.as_ref() {
            client_keys.insert(resolution.data_client_id.to_string());
        }
    }
    insert_outcome_group_source_client_keys(&mut client_keys, loaded)?;
    insert_gate_provider_client_keys(&mut client_keys, loaded)?;
    insert_realized_volatility_surface_client_keys(&mut client_keys, loaded, rv_scope);
    insert_iv_source_client_keys(&mut client_keys, loaded);
    Ok(client_keys)
}

fn insert_iv_source_client_keys(client_keys: &mut BTreeSet<String>, loaded: &LoadedBoltV3Config) {
    if let Some(iv_root) = loaded.root.iv.as_ref() {
        for profile in &iv_root.profiles {
            for source in &profile.sources {
                client_keys.insert(source.client_id.clone());
            }
        }
    }
}

fn insert_outcome_group_source_client_keys(
    client_keys: &mut BTreeSet<String>,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let plan = crate::bolt_v3_market_families::market_identity_plan_from_config(loaded).map_err(
        |source| BoltV3LiveNodeError::LiveTransportScope {
            reason: source.to_string(),
        },
    )?;
    let Some(sources) = loaded.root.outcome_group_sources.as_ref() else {
        return Ok(());
    };
    for target in crate::bolt_v3_market_families::outcome_group::target_plans(&plan) {
        for source_id in &target.group_sources {
            if let Some(source) = sources.iter().find(|source| source.source_id == *source_id)
                && source.enabled
            {
                client_keys.insert(source.client_id.to_string());
            }
        }
    }
    Ok(())
}

fn insert_gate_provider_client_keys(
    client_keys: &mut BTreeSet<String>,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    for strategy in &loaded.strategies {
        let Some(target) = target_gate_references(strategy)? else {
            continue;
        };
        let Some(subscriptions) = target.gate_subscriptions.as_ref() else {
            continue;
        };
        insert_gate_subscription_client_keys(
            client_keys,
            loaded.root.gate_providers.as_ref(),
            subscriptions,
            strategy.relative_path.as_str(),
        )?;
    }
    Ok(())
}

fn target_gate_references(
    strategy: &LoadedStrategy,
) -> Result<Option<TargetGateReferences>, BoltV3LiveNodeError> {
    if strategy.config.target.as_table().is_none() {
        return Ok(None);
    }
    strategy
        .config
        .target
        .clone()
        .try_into::<TargetGateReferences>()
        .map(Some)
        .map_err(|source| BoltV3LiveNodeError::LiveTransportScope {
            reason: format!(
                "strategy `{}` target.gate_subscriptions could not be parsed for trade transport scoping: {source}",
                strategy.relative_path
            ),
        })
}

fn insert_gate_subscription_client_keys(
    client_keys: &mut BTreeSet<String>,
    providers: Option<&BTreeMap<String, crate::bolt_v3_config::GateProviderBlock>>,
    subscriptions: &BTreeMap<String, crate::bolt_v3_market_families::TargetGateSubscription>,
    strategy_path: &str,
) -> Result<(), BoltV3LiveNodeError> {
    for subscription in subscriptions.values() {
        let _ = (
            subscription.required,
            &subscription.allowed_provider_kinds,
            &subscription.allowed_value_kinds,
            subscription.allow_no_resolution,
        );
        if let Some(provider_ids) = &subscription.allowed_provider_ids {
            for provider_id in provider_ids {
                insert_gate_provider_client_key(
                    client_keys,
                    providers,
                    provider_id,
                    strategy_path,
                )?;
            }
        }
        if let Some(provider_ids) = &subscription.provider_preference {
            for provider_id in provider_ids {
                insert_gate_provider_client_key(
                    client_keys,
                    providers,
                    provider_id,
                    strategy_path,
                )?;
            }
        }
        if let Some(mappings) = &subscription.market_mappings {
            for mapping in mappings {
                let _ = (
                    &mapping.family_key,
                    &mapping.market_class,
                    &mapping.resolution_kind,
                    &mapping.resolution_identity,
                    &mapping.value_kind,
                );
                if let Some(provider_id) = &mapping.provider_id {
                    insert_gate_provider_client_key(
                        client_keys,
                        providers,
                        provider_id,
                        strategy_path,
                    )?;
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct TargetGateReferences {
    gate_subscriptions:
        Option<BTreeMap<String, crate::bolt_v3_market_families::TargetGateSubscription>>,
}

fn insert_gate_provider_client_key(
    client_keys: &mut BTreeSet<String>,
    providers: Option<&BTreeMap<String, crate::bolt_v3_config::GateProviderBlock>>,
    provider_id: &str,
    strategy_path: &str,
) -> Result<(), BoltV3LiveNodeError> {
    let Some(providers) = providers else {
        return Err(BoltV3LiveNodeError::LiveTransportScope {
            reason: format!(
                "strategy `{strategy_path}` target.gate_subscriptions references provider_id `{provider_id}` but [gate_providers] is not configured"
            ),
        });
    };
    let Some(provider) = providers.get(provider_id) else {
        return Err(BoltV3LiveNodeError::LiveTransportScope {
            reason: format!(
                "strategy `{strategy_path}` target.gate_subscriptions references provider_id `{provider_id}` but [gate_providers.{provider_id}] is not configured"
            ),
        });
    };
    if let Some(client_id) = &provider.client_id {
        client_keys.insert(client_id.to_string());
    }
    Ok(())
}

fn insert_realized_volatility_surface_client_keys(
    client_keys: &mut BTreeSet<String>,
    loaded: &LoadedBoltV3Config,
    rv_scope: RealizedVolatilityTransportScope,
) {
    if !matches!(rv_scope, RealizedVolatilityTransportScope::Subscribed)
        || loaded.strategies.is_empty()
    {
        return;
    }
    let Some(surfaces) = loaded.root.realized_volatility_surfaces.as_ref() else {
        return;
    };
    for surface in surfaces.values() {
        client_keys.extend(
            surface
                .sources
                .iter()
                .filter(|source| source.enabled)
                .map(|source| source.data_client_id.to_string()),
        );
    }
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
        runtime.rebuild_position_sizer_from_nt_cache(startup_rebuild_observed_at_ns);
    // A no-open-order startup may legitimately recover nothing: NT only
    // populates the account/portfolio cache once its runner loop performs
    // startup reconciliation, and the live runtime feed re-seeds the
    // portfolio from on_account events after entry. Pre-existing open orders
    // are different: if they cannot be attributed to recovered reservation
    // metadata, submit admission would start with an unreconciled ledger and
    // could double-allocate capital, so fail closed before entering NT's
    // loop. This is a reconciliation guard, not the removed start gate.
    fail_closed_on_unreconciled_startup_rebuild(startup_rebuild)?;
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
    startup_rebuild: BoltV3SubmitPositionSizingRebuildDecision,
) -> Result<(), BoltV3LiveNodeError> {
    if !startup_rebuild.accepted && startup_rebuild.attempted_reservation_count > 0 {
        return Err(BoltV3LiveNodeError::StartupPositionSizerRebuild(
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

/// Test-friendly variant of [`build_bolt_v3_live_node`] which lets the caller
/// inject the forbidden-environment predicate and the SSM resolver. Production
/// code must use [`build_bolt_v3_live_node`], which applies the real credential
/// environment guard and invokes the real Amazon Web Services Systems Manager
/// resolver.
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
    let loss_policy = loss_governor_policy_from_loaded(loaded)?;
    let loss_halt_action_policy = loss_governor_halt_action_policy_from_loaded(loaded)?;
    let position_sizer = position_sizer_config_from_loaded(loaded)?;
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
        if loss_policy.is_none() && position_sizer.is_none() {
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
    let position_sizer_runtime_feed_config =
        position_sizer_runtime_feed_config_from_loaded(loaded, startup_observed_at_ns);
    let position_sizer_venue_spendability_source =
        position_sizer_venue_spendability_source_config_from_loaded(loaded)?;
    let submit_reservation_recovery = if position_sizer_runtime_feed_config.is_some() {
        submit_reservation_recovery_config_from_loaded(loaded)?
    } else {
        None
    };
    let submit_admission = Arc::new(
        BoltV3SubmitAdmissionState::new_with_live_submit_limits_and_optional_controls(
            decision_evidence.clone(),
            live_submit_approval_limits,
            loss_policy.clone(),
            position_sizer,
        ),
    );
    let (position_sizer_runtime_feed, position_sizer_runtime_feed_subscription) =
        match position_sizer_runtime_feed_config {
            Some(config) => {
                let feed = Arc::new(Mutex::new(PositionSizerRuntimeFeed::new(
                    config,
                    submit_admission.clone(),
                )));
                let subscription = subscribe_position_sizer_runtime_feed(feed.clone());
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
            crate::strategy_runtime_bindings::runtime_bindings(),
            strategy_execution_controls,
            decision_evidence.clone(),
            iv_runtime,
        )
    } else {
        register_bolt_v3_strategies_on_node_with_bindings(
            &mut node,
            loaded,
            resolved,
            crate::strategy_runtime_bindings::runtime_bindings(),
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
            loss_halt_action_policy,
            loss_runtime_feed,
            loss_runtime_feed_subscription,
            position_sizer_runtime_feed,
            position_sizer_runtime_feed_subscription,
            position_sizer_venue_spendability_source,
            submit_reservation_recovery,
        },
        iv_runtime,
        iv_event_bindings,
        resolved.redaction_values(),
    );
    runtime.refresh_position_sizer_venue_spendability_from_configured_source()?;
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

fn position_sizer_runtime_feed_config_from_loaded(
    loaded: &LoadedBoltV3Config,
    startup_observed_at_ns: u64,
) -> Option<PositionSizerRuntimeFeedConfig> {
    let pools = loaded.root.risk.capital_pools.as_ref()?;
    let pool = pools.iter().find(|pool| pool.enforce_submit_admission)?;
    let product = pool.prediction_market_binary.as_ref()?;
    Some(PositionSizerRuntimeFeedConfig {
        venue_id: pool.venue_id.clone(),
        account_id: pool.account_id,
        collateral_currency: pool.collateral_currency.clone(),
        product_state: ProductSizingSnapshot::PredictionMarketBinary(
            PredictionMarketSizingSnapshot {
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

fn position_sizer_venue_spendability_source_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<BoltV3PositionSizerVenueSpendabilitySourceConfig>, BoltV3LiveNodeError> {
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
    Ok(Some(BoltV3PositionSizerVenueSpendabilitySourceConfig {
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

fn position_sizer_venue_spendability_snapshot_from_source_config(
    config: &BoltV3PositionSizerVenueSpendabilitySourceConfig,
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
            "position sizer venue spendability source rejected: {error:?}"
        ))
    })
}

fn refresh_position_sizer_venue_spendability_from_source(
    feed: &Arc<Mutex<PositionSizerRuntimeFeed>>,
    config: &BoltV3PositionSizerVenueSpendabilitySourceConfig,
) -> Result<Option<BoltV3SubmitPositionSizingNtComponents>, BoltV3LiveNodeError> {
    let snapshot = position_sizer_venue_spendability_snapshot_from_source_config(config)?;
    let mut feed = feed.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(feed.on_venue_spendability_snapshot(snapshot))
}

fn position_sizer_config_from_loaded(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<BoltV3SubmitPositionSizerConfig>, BoltV3LiveNodeError> {
    let Some(pools) = loaded.root.risk.capital_pools.as_ref() else {
        return Ok(None);
    };
    let Some(pool) = pools.iter().find(|pool| pool.enforce_submit_admission) else {
        return Ok(None);
    };
    Ok(Some(BoltV3SubmitPositionSizerConfig {
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
        policy: sizing_policy_from_pool(pool)?,
        dedupe_retention_ns: pool.dedupe_retention_ns,
    }))
}

fn sizing_policy_from_pool(pool: &CapitalPoolBlock) -> Result<SizingPolicy, BoltV3LiveNodeError> {
    let sizing = &pool.sizing_policy;
    Ok(SizingPolicy {
        min_remaining_pool_balance: optional_pool_decimal(
            "risk.capital_pools.sizing_policy.min_remaining_pool_balance",
            sizing.min_remaining_pool_balance.as_deref(),
        )?,
        fee_slippage_policy: Some(FeeSlippagePolicy {
            max_fee_liability: required_pool_decimal(
                "risk.capital_pools.sizing_policy.fee_slippage.max_fee_liability",
                &sizing.fee_slippage.max_fee_liability,
            )?,
            max_slippage_liability: required_pool_decimal(
                "risk.capital_pools.sizing_policy.fee_slippage.max_slippage_liability",
                &sizing.fee_slippage.max_slippage_liability,
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

fn loss_governor_halt_action_handler_from_node(
    node: &LiveNode,
    loss_policy: LossGovernorPolicy,
    action_policy: LossGovernorHaltActionPolicy,
) -> LossGovernorHaltActionHandler {
    let risk_engine = node.kernel().risk_engine().clone();
    Rc::new(move |snapshot, now_ns| {
        let decision = evaluate_loss_admission(&loss_policy, snapshot, now_ns);
        let current_state = risk_engine.borrow().trading_state();
        if current_state == TradingState::Active && decision.accepted {
            return;
        }

        if let Some(target_state) =
            next_loss_governor_trading_state(&action_policy, current_state, &decision)
        {
            risk_engine.borrow_mut().set_trading_state(target_state);
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

/// Translates a validated bolt-v3 config into an NT-native
/// [`LiveNodeBuilder`] with no clients added. Field translation goes
/// through [`make_live_node_config`] so the bolt-v3 → NT field mapping
/// has a single source of truth that the existing per-field tests can
/// keep exercising.
pub fn make_bolt_v3_live_node_builder(
    loaded: &LoadedBoltV3Config,
) -> Result<LiveNodeBuilder, BoltV3LiveNodeBuilderError> {
    let cfg = make_live_node_config(loaded);
    make_bolt_v3_live_node_builder_from_config(cfg)
}

fn make_bolt_v3_live_node_builder_from_config(
    cfg: LiveNodeConfig,
) -> Result<LiveNodeBuilder, BoltV3LiveNodeBuilderError> {
    LiveNodeBuilder::from_config(cfg)
        .map_err(|source| BoltV3LiveNodeBuilderError::BuilderConstruction { source })
}

pub fn make_live_node_config(loaded: &LoadedBoltV3Config) -> LiveNodeConfig {
    let trader_id = loaded.root.trader_id;
    let environment = loaded.root.runtime.mode;
    let mut module_level: AHashMap<Ustr, LevelFilter> = AHashMap::new();
    for module_path in bolt_v3_providers::credential_log_modules() {
        module_level.insert(Ustr::from(module_path), LevelFilter::Warn);
    }
    let logging = LoggerConfig {
        stdout_level: nautilus_common::logging::map_log_level_to_filter(
            loaded.root.logging.stdout_level,
        ),
        fileout_level: nautilus_common::logging::map_log_level_to_filter(
            loaded.root.logging.fileout_level,
        ),
        component_level: AHashMap::new(),
        module_level,
        log_components_only: false,
        is_colored: true,
        print_config: false,
        use_tracing: false,
        bypass_logging: false,
        file_config: None,
        clear_log_file: false,
    };
    let nautilus = &loaded.root.nautilus;
    let data = &nautilus.data_engine;
    let data_engine = nautilus_live::config::LiveDataEngineConfig {
        time_bars_build_with_no_updates: data.time_bars_build_with_no_updates,
        time_bars_timestamp_on_close: data.time_bars_timestamp_on_close,
        time_bars_skip_first_non_full_bar: data.time_bars_skip_first_non_full_bar,
        time_bars_interval_type: bar_interval_type_from_str(&data.time_bars_interval_type),
        time_bars_build_delay: data.time_bars_build_delay,
        // Bolt stores this as a BTreeMap for deterministic config/debug output;
        // NT's live data config consumes the same aggregation/nanosecond pairs as a HashMap.
        time_bars_origin_offset: data.time_bars_origins.clone().into_iter().collect(),
        validate_data_sequence: data.validate_data_sequence,
        buffer_deltas: data.buffer_deltas,
        emit_quotes_from_book: data.emit_quotes_from_book,
        emit_quotes_from_book_depths: data.emit_quotes_from_book_depths,
        external_clients: configured_external_clients(&data.external_clients),
        debug: data.debug,
        graceful_shutdown_on_error: data.graceful_shutdown_on_error,
        qsize: data.qsize,
    };
    let exec = &nautilus.exec_engine;
    let reconciliation_lookback_mins = u32_zero_as_none(exec.reconciliation_lookback_mins);
    let exec_engine = nautilus_live::config::LiveExecEngineConfig {
        load_cache: exec.load_cache,
        snapshot_orders: exec.snapshot_orders,
        snapshot_positions: exec.snapshot_positions,
        snapshot_positions_interval_secs: u64_zero_as_none_f64(
            exec.snapshot_positions_interval_secs,
        ),
        external_clients: configured_external_clients(&exec.external_clients),
        debug: exec.debug,
        reconciliation: exec.reconciliation,
        reconciliation_lookback_mins,
        // `f64` is lossless for all practical delay values (< 2^53 seconds).
        reconciliation_startup_delay_secs: exec.reconciliation_startup_delay_secs as f64,
        reconciliation_instrument_ids: non_empty_strings(&exec.reconciliation_instrument_ids),
        filter_unclaimed_external_orders: exec.filter_unclaimed_external_orders,
        filter_position_reports: exec.filter_position_reports,
        filtered_client_order_ids: non_empty_strings(&exec.filtered_client_order_ids),
        generate_missing_orders: exec.generate_missing_orders,
        inflight_check_interval_ms: exec.inflight_check_interval_ms,
        inflight_check_threshold_ms: exec.inflight_check_threshold_ms,
        inflight_check_retries: exec.inflight_check_retries,
        open_check_interval_secs: u64_zero_as_none_f64(exec.open_check_interval_secs),
        open_check_lookback_mins: u32_zero_as_none(exec.open_check_lookback_mins),
        open_check_threshold_ms: exec.open_check_threshold_ms,
        open_check_missing_retries: exec.open_check_missing_retries,
        open_check_open_only: exec.open_check_open_only,
        max_single_order_queries_per_cycle: exec.max_single_order_queries_per_cycle,
        single_order_query_delay_ms: exec.single_order_query_delay_ms,
        position_check_interval_secs: u64_zero_as_none_f64(exec.position_check_interval_secs),
        position_check_lookback_mins: exec.position_check_lookback_mins,
        position_check_threshold_ms: exec.position_check_threshold_ms,
        position_check_retries: exec.position_check_retries,
        purge_closed_orders_interval_mins: u32_zero_as_none(exec.purge_closed_orders_interval_mins),
        purge_closed_orders_buffer_mins: u32_zero_as_none(exec.purge_closed_orders_buffer_mins),
        purge_closed_positions_interval_mins: u32_zero_as_none(
            exec.purge_closed_positions_interval_mins,
        ),
        purge_closed_positions_buffer_mins: u32_zero_as_none(
            exec.purge_closed_positions_buffer_mins,
        ),
        purge_account_events_interval_mins: u32_zero_as_none(
            exec.purge_account_events_interval_mins,
        ),
        purge_account_events_lookback_mins: u32_zero_as_none(
            exec.purge_account_events_lookback_mins,
        ),
        purge_from_database: exec.purge_from_database,
        own_books_audit_interval_secs: u64_zero_as_none_f64(exec.own_books_audit_interval_secs),
        graceful_shutdown_on_error: exec.graceful_shutdown_on_error,
        qsize: exec.qsize,
        allow_overfills: exec.allow_overfills,
        manage_own_order_books: exec.manage_own_order_books,
    };
    let risk_engine = nautilus_live::config::LiveRiskEngineConfig {
        // Mandated safety invariant: the NT live risk engine must never be
        // bypassed. This is pinned in code with no config knob so no TOML edit
        // or operator override can disable pre-trade risk checks.
        bypass: false,
        max_order_submit_rate: loaded.root.risk.nautilus.max_order_submit_rate.clone(),
        max_order_modify_rate: loaded.root.risk.nautilus.max_order_modify_rate.clone(),
        // Bolt stores this as a BTreeMap for deterministic config/debug output;
        // NT's live risk config consumes the same string pairs as a HashMap.
        max_notional_per_order: loaded
            .root
            .risk
            .nautilus
            .max_notional_per_order
            .clone()
            .into_iter()
            .collect(),
        debug: loaded.root.risk.nautilus.debug,
        graceful_shutdown_on_error: loaded.root.risk.nautilus.graceful_shutdown_on_error,
        qsize: loaded.root.risk.nautilus.qsize,
    };

    // Explicit struct literal: upstream NT `LiveNodeConfig` field additions must be
    // considered here instead of silently inherited through `Default`.
    LiveNodeConfig {
        environment,
        trader_id,
        load_state: nautilus.load_state,
        save_state: nautilus.save_state,
        logging,
        instance_id: None,
        timeout_connection: Duration::from_secs(nautilus.timeout_connection_secs),
        timeout_reconciliation: Duration::from_secs(nautilus.timeout_reconciliation_secs),
        timeout_portfolio: Duration::from_secs(nautilus.timeout_portfolio_secs),
        timeout_disconnection: Duration::from_secs(nautilus.timeout_disconnection_secs),
        delay_post_stop: Duration::from_secs(nautilus.delay_post_stop_secs),
        timeout_shutdown: Duration::from_secs(nautilus.timeout_shutdown_secs),
        cache: None,
        msgbus: None,
        portfolio: None,
        emulator: None,
        streaming: None,
        event_store: None,
        loop_debug: false,
        data_engine,
        risk_engine,
        exec_engine,
        data_clients: HashMap::new(),
        exec_clients: HashMap::new(),
        plugins: Vec::new(),
    }
}

fn u32_zero_as_none(value: u32) -> Option<u32> {
    (value != 0).then_some(value)
}

fn u64_zero_as_none_f64(value: u64) -> Option<f64> {
    (value != 0).then_some(value as f64)
}

fn non_empty_strings(values: &[String]) -> Option<Vec<String>> {
    (!values.is_empty()).then(|| values.to_vec())
}

fn configured_external_clients(values: &[ClientId]) -> Option<Vec<ClientId>> {
    (!values.is_empty()).then(|| values.to_vec())
}

/// Caller must run root validation first so the string is a valid NT `BarIntervalType`.
fn bar_interval_type_from_str(value: &str) -> BarIntervalType {
    BarIntervalType::from_str(value).expect("root validation must accept data bar interval type")
}

pub fn wire_bolt_v3_runtime_capture(
    node: &LiveNode,
    stop_handle: LiveNodeHandle,
    loaded: &LoadedBoltV3Config,
) -> Result<NtRuntimeCaptureGuards> {
    wire_nt_runtime_capture(
        node,
        stop_handle,
        &loaded.root.persistence.catalog_directory,
        loaded.root.persistence.streaming.flush_interval_ms,
        loaded
            .root
            .persistence
            .runtime_capture_start_poll_interval_ms,
        None,
    )
}

/// Bolt-v3 controlled-connect boundary.
///
/// Drives the pinned NautilusTrader controlled-connect API
/// (`NautilusKernel::connect_data_clients` followed by
/// `NautilusKernel::connect_exec_clients`) on every NT data and
/// execution client that the bolt-v3 client-registration boundary added
/// to `node`, bounded by the bolt-v3
/// `nautilus.timeout_connection_secs` value from `loaded`.
///
/// This boundary is **opt-in**: `build_bolt_v3_live_node` and its
/// `_with` / `_with_summary` siblings deliberately do not invoke it.
/// A caller must explicitly call this function on a node previously
/// returned by one of those builders. In a bolt-v3-only process, NT's
/// first-wins logger is initialized by the bolt-v3 `LoggerConfig`
/// passed through `LiveNodeBuilder::build`, so the
/// provider-owned credential log module filters remain active during
/// connect.
/// The production bolt-v3 entrypoint preserves that ordering.
///
/// This boundary is **bounded**: the dispatched engine-level connect
/// futures are wrapped in `tokio::time::timeout` driven by
/// `nautilus.timeout_connection_secs`. If the bound elapses before
/// both engines finish dispatching connect to their registered clients
/// the function returns [`BoltV3LiveNodeError::ConnectTimeout`] and
/// the `LiveNode` is left in whatever partially-connected state NT
/// produced; the caller owns subsequent disconnect/teardown via
/// [`disconnect_bolt_v3_clients`].
///
/// This boundary is **dispatch + connected check**, not NT cache or
/// instrument readiness. The pinned NT `DataEngine::connect` and
/// `ExecutionEngine::connect` dispatchers swallow individual client
/// `connect()` errors and only log them, so after the dispatch
/// returns the bolt-v3 boundary consults
/// `NautilusKernel::check_engines_connected()` to ensure every
/// registered client transitioned to `is_connected`. If that check
/// returns false, the boundary returns
/// [`BoltV3LiveNodeError::ConnectIncomplete`] rather than `Ok(())`.
/// The boundary does **not** copy or reimplement NT private drain or
/// flush logic, and it does not gate on NT cache contents or
/// instrument-availability checks; that readiness is owned by a
/// future slice.
///
/// This boundary is **no-trade**: it never enters NT's runner loop
/// and never invokes NT's trader entrypoint, so no strategy actor is
/// activated, no reconciliation runs, and the runner loop is never
/// entered. `NodeState` therefore remains in whatever state the node
/// was in before the call (typically `Idle`). The boundary does not
/// register strategies, select markets, construct orders, submit
/// orders, or invoke any user-level subscription API.
///
/// Errors from individual NT client `connect()` calls are surfaced
/// via NT's logger (the engine-level dispatchers in
/// `nautilus_data::engine::DataEngine::connect` and
/// `nautilus_execution::engine::ExecutionEngine::connect` log
/// individual `Err` values rather than propagating them). The bolt-v3
/// boundary returns `Ok(())` only when both dispatchers have returned
/// within the configured bound **and**
/// `kernel.check_engines_connected()` returns true.
pub async fn connect_bolt_v3_clients(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let timeout_secs = loaded.root.nautilus.timeout_connection_secs;
    let bound = Duration::from_secs(timeout_secs);
    let connect = async {
        let kernel = node.kernel_mut();
        kernel.connect_data_clients().await;
        kernel.connect_exec_clients().await;
        kernel.check_engines_connected()
    };
    match tokio::time::timeout(bound, connect).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(BoltV3LiveNodeError::ConnectIncomplete),
        Err(_) => Err(BoltV3LiveNodeError::ConnectTimeout { timeout_secs }),
    }
}

/// Bolt-v3 controlled-disconnect boundary.
///
/// Drives the pinned NautilusTrader controlled-disconnect API
/// (`NautilusKernel::disconnect_clients`) on every NT data and
/// execution client previously added through the bolt-v3
/// client-registration boundary, bounded by the bolt-v3
/// `nautilus.timeout_disconnection_secs` value from `loaded`.
///
/// Recovery counterpart to [`connect_bolt_v3_clients`]: after a
/// `ConnectTimeout` or `ConnectIncomplete` the caller is expected to
/// invoke this function to drain whatever partially-connected NT
/// clients survive, again under a bounded timeout.
///
/// This boundary is **bounded**: NT's
/// `kernel.disconnect_clients()` future is wrapped in
/// `tokio::time::timeout`. On the bound elapsing, the function
/// returns [`BoltV3LiveNodeError::DisconnectTimeout`] with the
/// configured bound. On NT's engine-level disconnect aggregator
/// surfacing an `Err(..)`, the function returns
/// [`BoltV3LiveNodeError::DisconnectFailed`] wrapping the NT
/// `anyhow::Error`. Pinned NT disconnects data clients before
/// execution clients and can short-circuit on a data-client error; a
/// `DisconnectFailed` therefore leaves cleanup state indeterminate and
/// production recovery should rebuild a fresh `LiveNode`.
///
/// This boundary is **no-trade**: it never enters NT's runner loop,
/// never invokes NT's trader entrypoint, never registers strategies,
/// never selects markets, never constructs orders, never submits
/// orders, and never invokes any user-level subscription API. It
/// does not call `LiveNode::stop`; the bolt-v3 LiveNode remains
/// outside NT's runner-driven lifecycle. The boundary does **not**
/// copy or reimplement NT private drain or flush logic.
pub async fn disconnect_bolt_v3_clients(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let timeout_secs = loaded.root.nautilus.timeout_disconnection_secs;
    let bound = Duration::from_secs(timeout_secs);
    let disconnect = async { node.kernel_mut().disconnect_clients().await };
    match tokio::time::timeout(bound, disconnect).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(BoltV3LiveNodeError::DisconnectFailed(error)),
        Err(_) => Err(BoltV3LiveNodeError::DisconnectTimeout { timeout_secs }),
    }
}

#[cfg(test)]
mod tests {
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
    use crate::bolt_v3_loss_governor::LossSnapshot;
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

    #[test]
    fn startup_rebuild_recovers_known_submit_reservation_from_nt_cache() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let metadata = fixture_submit_reservation_metadata(
            "startup-known-client-order",
            "condition-fixture-yes.POLYMARKET",
            "buy",
            "10",
            "0.4",
            "0.3",
            "4.3",
        );
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");

        assert_eq!(runtime.position_sizer_reconciled(), Some(false));
        write_submit_reservation_metadata(&loaded, &metadata);
        seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 100.0);
        seed_accepted_open_limit_order(
            &runtime,
            generic_limit_order(
                "startup-known-client-order",
                "condition-fixture-yes.POLYMARKET",
                OrderSide::Buy,
                Quantity::from(6),
                Price::from("0.40"),
            ),
            "POLYMARKET-001",
        );

        let rebuild = runtime.rebuild_position_sizer_from_nt_cache(2_000);

        assert_eq!(
            rebuild,
            BoltV3SubmitPositionSizingRebuildDecision {
                accepted: true,
                reason: None,
                attempted_reservation_count: 1,
                rebuilt_reservation_count: 1,
                live_reserved_liability: Decimal::new(27, 1),
                missing_nt_account_cache_balance: None,
            }
        );
        assert_eq!(runtime.position_sizer_reconciled(), Some(true));
        assert_eq!(
            runtime
                .submit_admission
                .position_sizer_live_reserved_liability(),
            Some(Decimal::new(27, 1))
        );
        assert!(
            runtime
                .submit_admission
                .position_sizer_has_live_reservation("startup-known-client-order"),
            "startup rebuild must install the recovered reservation for later fill/cancel release"
        );
    }

    #[test]
    fn startup_rebuild_stays_closed_for_unknown_nt_cache_order() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");
        seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 100.0);
        seed_accepted_open_limit_order(
            &runtime,
            generic_limit_order(
                "startup-unknown-client-order",
                "condition-fixture-yes.POLYMARKET",
                OrderSide::Buy,
                Quantity::from(6),
                Price::from("0.40"),
            ),
            "POLYMARKET-001",
        );

        let rebuild = runtime.rebuild_position_sizer_from_nt_cache(2_000);

        assert_eq!(
            rebuild,
            BoltV3SubmitPositionSizingRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count: 1,
                rebuilt_reservation_count: 0,
                live_reserved_liability: Decimal::ZERO,
                missing_nt_account_cache_balance: None,
            }
        );
        assert_eq!(runtime.position_sizer_reconciled(), Some(false));
        assert!(
            !runtime
                .submit_admission
                .position_sizer_has_live_reservation("startup-unknown-client-order")
        );
    }

    #[test]
    fn startup_rebuild_guard_aborts_before_live_node_run_for_unattributed_open_orders() {
        let rebuild = BoltV3SubmitPositionSizingRebuildDecision {
            accepted: false,
            reason: Some(ReservationRejectionReason::MissingEvidence),
            attempted_reservation_count: 1,
            rebuilt_reservation_count: 0,
            live_reserved_liability: Decimal::ZERO,
            missing_nt_account_cache_balance: None,
        };

        let error = fail_closed_on_unreconciled_startup_rebuild(rebuild.clone())
            .expect_err("unattributed startup open orders must abort before NT runner entry");

        match error {
            BoltV3LiveNodeError::StartupPositionSizerRebuild(decision) => {
                assert_eq!(decision, rebuild);
            }
            other => panic!("unexpected startup rebuild guard error: {other:?}"),
        }
    }

    #[test]
    fn startup_rebuild_reports_missing_nt_account_cache_balance() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");

        let rebuild = runtime.rebuild_position_sizer_from_nt_cache(2_000);

        assert_eq!(
            rebuild.missing_nt_account_cache_balance,
            Some(BoltV3SubmitPositionSizingMissingNtAccountCacheBalance {
                account_id: "POLYMARKET-001".to_string(),
                collateral_currency: "PUSD".to_string(),
            })
        );
        assert_eq!(runtime.position_sizer_reconciled(), Some(false));

        let feed = runtime
            .position_sizer_runtime_feed
            .as_ref()
            .expect("fixture should configure position-sizer runtime feed");
        let account_state = account_state_event("POLYMARKET-001", "PUSD", 100.0, 100.0, 2_100);
        assert!(
            feed.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .on_account_state(&account_state)
                .is_some(),
            "account state should still publish sizing components once collateral facts arrive"
        );
        assert_eq!(
            runtime.position_sizer_reconciled(),
            Some(false),
            "missing startup account cache means the pre-run empty order cache is not authoritative"
        );
        let state = runtime
            .submit_admission
            .position_sizer_state_snapshot()
            .expect("account state should publish an unreconciled sizing state");
        assert_eq!(state.order_lifecycle.source, "nt_order_lifecycle_seed");
        assert!(!state.order_lifecycle.all_open_orders_attributed);
    }

    #[test]
    fn live_node_build_does_not_apply_loss_halt_before_first_trusted_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");

        assert_eq!(runtime.nt_risk_trading_state(), TradingState::Active);
    }

    #[test]
    fn manual_recovery_evidence_clears_live_reducing_state_after_fresh_snapshot() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");
        runtime
            .node
            .kernel()
            .risk_engine()
            .borrow_mut()
            .set_trading_state(TradingState::Reducing);
        runtime.submit_admission.update_loss_snapshot(LossSnapshot {
            source: "nt_loss_runtime_feed".to_string(),
            observed_at_ns: 2_000,
            per_trade_pnl: Some(Decimal::ZERO),
            daily_pnl: Some(Decimal::ZERO),
            rolling_pnl: Some(Decimal::ZERO),
            current_equity: Some(Decimal::new(100, 0)),
            peak_equity: Some(Decimal::new(100, 0)),
        });
        let evidence = LossGovernorManualRecoveryEvidence::new(
            "operator-primary",
            "loss-governor/manual-recovery.json",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            2_050,
            256,
        )
        .expect("bounded manual recovery evidence should validate");

        let target = runtime.apply_loss_governor_manual_recovery(&evidence, 2_100);

        assert_eq!(target, Some(TradingState::Active));
        assert_eq!(runtime.nt_risk_trading_state(), TradingState::Active);
    }

    #[test]
    fn startup_rebuild_seeds_nt_cached_free_collateral_when_balance_has_locked_amount() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");

        // Helper writes NT AccountBalance as (total, locked, free): total=100, locked=40, free=60.
        seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 60.0);
        {
            let account_id = AccountId::from("POLYMARKET-001");
            let cache = runtime.node.kernel().cache();
            let cache = cache.borrow();
            let account = cache
                .account_owned(&account_id)
                .expect("seeded account should be present in NT cache");
            let balances = account.balances();
            let balance = balances
                .values()
                .find(|balance| balance.currency.code.as_str() == "PUSD")
                .expect("seeded collateral balance should be present in NT cache");
            assert_eq!(balance.total.as_decimal(), Decimal::new(100, 0));
            assert_eq!(balance.locked.as_decimal(), Decimal::new(40, 0));
            assert_eq!(balance.free.as_decimal(), Decimal::new(60, 0));
        }

        let rebuild = runtime.rebuild_position_sizer_from_nt_cache(2_000);

        assert_eq!(rebuild.missing_nt_account_cache_balance, None);
        assert_eq!(runtime.position_sizer_reconciled(), Some(true));
        let state = runtime
            .submit_admission
            .position_sizer_state_snapshot()
            .expect("startup rebuild should seed position sizing state");
        assert_eq!(state.portfolio.free_collateral, Decimal::new(60, 0));
        assert_eq!(state.portfolio.total_equity, Decimal::new(100, 0));
        assert_ne!(
            state.portfolio.free_collateral, state.portfolio.total_equity,
            "fixture must prove locked collateral is not treated as spendable"
        );
        match state.product_state {
            ProductSizingSnapshot::PredictionMarketBinary(snapshot) => {
                assert_eq!(snapshot.collateral_allowance, Decimal::new(60, 0));
            }
        }
    }

    #[test]
    fn startup_rebuild_rejects_known_metadata_when_open_quantity_exceeds_submitted() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let loaded = loaded_config_with_submit_sizer_recovery(temp.path());
        let metadata = fixture_submit_reservation_metadata(
            "startup-overopen-client-order",
            "condition-fixture-yes.POLYMARKET",
            "buy",
            "10",
            "0.4",
            "0.3",
            "4.3",
        );
        let runtime = build_bolt_v3_live_node_with(&loaded, |_| false, fake_bolt_v3_resolver)
            .expect("fixture v3 LiveNode should build");

        write_submit_reservation_metadata(&loaded, &metadata);
        seed_cached_account_state(&runtime, "POLYMARKET-001", "PUSD", 100.0, 100.0);
        seed_accepted_open_limit_order(
            &runtime,
            generic_limit_order(
                "startup-overopen-client-order",
                "condition-fixture-yes.POLYMARKET",
                OrderSide::Buy,
                Quantity::from(11),
                Price::from("0.40"),
            ),
            "POLYMARKET-001",
        );

        let rebuild = runtime.rebuild_position_sizer_from_nt_cache(2_000);

        assert_eq!(
            rebuild,
            BoltV3SubmitPositionSizingRebuildDecision {
                accepted: false,
                reason: Some(ReservationRejectionReason::MissingEvidence),
                attempted_reservation_count: 1,
                rebuilt_reservation_count: 0,
                live_reserved_liability: Decimal::ZERO,
                missing_nt_account_cache_balance: None,
            }
        );
        assert_eq!(runtime.position_sizer_reconciled(), Some(false));
        assert!(
            !runtime
                .submit_admission
                .position_sizer_has_live_reservation("startup-overopen-client-order")
        );
    }

    #[test]
    fn nt_limit_order_snapshot_maps_to_generic_open_order_evidence() {
        let order = generic_limit_order(
            "client-order-1",
            "instrument-yes.VENUE-A",
            OrderSide::Buy,
            Quantity::from(10),
            Price::from("0.40"),
        );

        let evidence = nt_open_order_evidence_from_order(&order, 1_000)
            .expect("bounded NT limit order should produce generic open-order evidence");

        assert_eq!(evidence.client_order_id, "client-order-1");
        assert_eq!(evidence.instrument_id, "instrument-yes.VENUE-A");
        assert_eq!(evidence.side, BoltV3CompiledOrderSide::Buy);
        assert_eq!(evidence.open_quantity, Decimal::new(10, 0));
        assert_eq!(evidence.limit_price, Decimal::new(4, 1));
        assert_eq!(evidence.observed_at_ns, 1_000);
        assert_eq!(evidence.evidence_label, "nt_open_order_cache");
    }

    #[test]
    fn nt_non_limit_order_snapshot_is_not_sizing_evidence() {
        let order = generic_market_order(
            "client-order-1",
            "instrument-yes.VENUE-A",
            OrderSide::Buy,
            Quantity::from(10),
        );

        assert!(nt_open_order_evidence_from_order(&order, 1_000).is_none());
    }

    fn loaded_config_with_submit_sizer_recovery(temp_path: &std::path::Path) -> LoadedBoltV3Config {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded.strategies.clear();
        loaded
            .root
            .risk
            .capital_pools
            .as_mut()
            .expect("fixture should configure capital pools")[0]
            .enforce_submit_admission = true;
        loaded.root.persistence.catalog_directory = temp_path.to_string_lossy().to_string();
        loaded
            .root
            .persistence
            .decision_evidence
            .recovery_evidence_max_bytes = Some(100_000);
        loaded
    }

    fn fixture_submit_reservation_metadata(
        client_order_id: &str,
        instrument_id: &str,
        side: &str,
        submitted_quantity: &str,
        liability_factor: &str,
        additive_liability: &str,
        reserved_liability: &str,
    ) -> BoltV3SubmitReservationMetadataEvidence {
        BoltV3SubmitReservationMetadataEvidence {
            client_order_id: client_order_id.to_string(),
            submit_reservation_id: format!("{client_order_id}#submit"),
            venue_id: "POLYMARKET".to_string(),
            account_id: "POLYMARKET-001".to_string(),
            product_kind: "prediction_market_binary".to_string(),
            collateral_currency: "PUSD".to_string(),
            capital_pool_id: "polymarket-prediction-live".to_string(),
            collateral_group_id: "condition-fixture".to_string(),
            instrument_id: instrument_id.to_string(),
            side: side.to_string(),
            submitted_quantity: submitted_quantity.to_string(),
            liability_factor: liability_factor.to_string(),
            additive_liability: additive_liability.to_string(),
            reserved_liability: reserved_liability.to_string(),
            observed_at_ns: 1_000,
            source: "submit_admission".to_string(),
        }
    }

    fn write_submit_reservation_metadata(
        loaded: &LoadedBoltV3Config,
        metadata: &BoltV3SubmitReservationMetadataEvidence,
    ) {
        let writer = JsonlBoltV3DecisionEvidenceWriter::from_loaded_config(loaded)
            .expect("decision evidence writer should open");
        writer
            .record_submit_reservation_metadata(metadata)
            .expect("submit reservation metadata should write");
    }

    fn fake_bolt_v3_resolver(_region: &str, path: &str) -> Result<String, &'static str> {
        match path {
            "/bolt/polymarket_main/private_key" => Ok(
                "0x4242424242424242424242424242424242424242424242424242424242424242".to_string(),
            ),
            "/bolt/polymarket_main/api_key" => Ok("polymarket-api-key".to_string()),
            "/bolt/polymarket_main/api_secret" => Ok("YWJj".to_string()),
            "/bolt/polymarket_main/passphrase" => Ok("polymarket-passphrase".to_string()),
            "/bolt/binance_reference/api_key" => Ok("binance-api-key".to_string()),
            "/bolt/binance_reference/api_secret" => {
                Ok("MC4CAQAwBQYDK2VwBCIEIAABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4f".to_string())
            }
            _ => Err("unexpected SSM path requested by bolt-v3 fake resolver"),
        }
    }

    fn seed_cached_account_state(
        runtime: &BoltV3LiveNodeRuntime,
        account_id: &str,
        currency_code: &str,
        total: f64,
        free: f64,
    ) {
        let account_state = account_state_event(account_id, currency_code, total, free, 1);
        runtime
            .node
            .kernel()
            .cache()
            .borrow_mut()
            .update_account_state(&account_state)
            .expect("NT cache should apply account state");
    }

    fn account_state_event(
        account_id: &str,
        currency_code: &str,
        total: f64,
        free: f64,
        timestamp_ns: u64,
    ) -> AccountState {
        let currency = test_currency(currency_code);
        AccountState::new(
            AccountId::from(account_id),
            AccountType::Cash,
            vec![AccountBalance::new(
                Money::new(total, currency),
                Money::new(total - free, currency),
                Money::new(free, currency),
            )],
            vec![],
            true,
            UUID4::default(),
            UnixNanos::from(timestamp_ns),
            UnixNanos::from(timestamp_ns),
            Some(currency),
        )
    }

    fn test_currency(currency_code: &str) -> Currency {
        if currency_code == "PUSD" {
            return Currency::new("PUSD", 2, 0, "Test PUSD", CurrencyType::Fiat);
        }
        Currency::from(currency_code)
    }

    fn seed_accepted_open_limit_order(
        runtime: &BoltV3LiveNodeRuntime,
        order: OrderAny,
        account_id: &str,
    ) {
        let cache = runtime.node.kernel().cache();
        let mut cache = cache.borrow_mut();
        cache
            .add_order(
                order.clone(),
                None,
                Some(ClientId::from("polymarket_main")),
                false,
            )
            .expect("NT cache should accept initialized order");
        cache
            .update_order(&OrderEventAny::Submitted(OrderSubmitted::new(
                order.trader_id(),
                order.strategy_id(),
                order.instrument_id(),
                order.client_order_id(),
                AccountId::from(account_id),
                UUID4::default(),
                UnixNanos::from(1),
                UnixNanos::from(1),
            )))
            .expect("NT cache should apply submitted event");
        cache
            .update_order(&OrderEventAny::Accepted(OrderAccepted::new(
                order.trader_id(),
                order.strategy_id(),
                order.instrument_id(),
                order.client_order_id(),
                VenueOrderId::from("venue-order-startup"),
                AccountId::from(account_id),
                UUID4::default(),
                UnixNanos::from(2),
                UnixNanos::from(2),
                false,
            )))
            .expect("NT cache should apply accepted event");
    }

    fn generic_limit_order(
        client_order_id: &str,
        instrument_id: &str,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
    ) -> OrderAny {
        OrderAny::Limit(
            LimitOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from(instrument_id),
                ClientOrderId::from(client_order_id),
                order_side,
                quantity,
                price,
                TimeInForce::Gtc,
                None,
                false,
                false,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                UUID4::default(),
                UnixNanos::from(1),
            )
            .expect("generic limit order should be valid"),
        )
    }

    fn generic_market_order(
        client_order_id: &str,
        instrument_id: &str,
        order_side: OrderSide,
        quantity: Quantity,
    ) -> OrderAny {
        OrderAny::Market(
            MarketOrder::new_checked(
                TraderId::from("TRADER-001"),
                StrategyId::from("strategy-a"),
                InstrumentId::from(instrument_id),
                ClientOrderId::from(client_order_id),
                order_side,
                quantity,
                TimeInForce::Ioc,
                UUID4::default(),
                UnixNanos::from(1),
                false,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .expect("generic market order should be valid"),
        )
    }

    #[test]
    fn live_node_adapter_mapping_consumes_hyperliquid_live_submit_approval_artifact() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
        let product_proof_sha256 = write_hyperliquid_test_product_submit_proof(&product_proof_path);
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = fixture_loaded_config_with_hyperliquid_standard_perps_route();
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                product_proof_path.display(),
                product_proof_sha256
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: product_proof_path.display().to_string(),
                    artifact_sha256: product_proof_sha256,
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let bundle = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect("production live-node mapping should consume approval and map execution");

        assert!(
            bundle
                .configs
                .clients
                .get("hyperliquid_perps")
                .and_then(|client| client.execution.as_ref())
                .is_some(),
            "consumed approval should reach the execution adapter mapper"
        );
        let approval_limits = bundle
            .live_submit_approval_limits
            .get("hyperliquid_perps")
            .expect("consumed Hyperliquid approval should carry submit-admission limits");
        assert_eq!(approval_limits.max_order_count, 2);
        assert_eq!(
            approval_limits.max_order_notional,
            Decimal::from_str_exact("25.00").expect("expected decimal should parse")
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("consumed approval should still read"),
        )
        .expect("consumed approval JSON should parse");
        assert_eq!(persisted["used_at"], now);

        let error = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now + 1,
            &build_head_sha,
        )
        .expect_err("persisted consumption must prevent approval reuse");
        assert!(
            error.to_string().contains("used_at"),
            "reuse failure should identify the spent approval field: {error}"
        );
    }

    #[test]
    fn live_node_without_hyperliquid_execution_target_does_not_select_or_consume_approval() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
        let product_proof_sha256 = write_hyperliquid_test_product_submit_proof(&product_proof_path);
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = fixture_loaded_config();
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                product_proof_path.display(),
                product_proof_sha256
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: product_proof_path.display().to_string(),
                    artifact_sha256: product_proof_sha256,
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let approvals = load_provider_live_submit_approvals_for_live_node(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect("no active Hyperliquid execution target should leave approvals unselected");

        assert!(
            approvals.is_empty(),
            "a configured single-surface client must not auto-select live-submit without an active execution target"
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
        )
        .expect("unconsumed approval JSON should parse");
        assert_eq!(
            persisted["used_at"],
            serde_json::Value::Null,
            "absence of a Hyperliquid execution target must not spend one-time approval artifacts"
        );
    }

    #[test]
    fn live_node_with_only_non_hyperliquid_routes_does_not_select_or_consume_hyperliquid_approval()
    {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
        let product_proof_sha256 = write_hyperliquid_test_product_submit_proof(&product_proof_path);
        let private_key = format!("0x{}", "1".repeat(64));
        // Load the fixture with its real strategies, which route execution to a
        // non-Hyperliquid client (polymarket_main), then add an unreferenced
        // Hyperliquid execution client. This exercises the
        // active_target_surfaces_for_client filter on the realistic path where a
        // strategy is active but targets a different venue.
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded.config_bundle_checksum = "b".repeat(64);
        assert!(
            !loaded.strategies.is_empty(),
            "fixture must route at least one strategy so this exercises the routes-elsewhere path"
        );
        assert!(
            loaded.strategies.iter().all(|strategy| {
                strategy.config.execution_client_id != ClientId::from("hyperliquid_perps")
            }),
            "fixture strategies must route to a non-Hyperliquid execution client for this test"
        );
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                product_proof_path.display(),
                product_proof_sha256
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: product_proof_path.display().to_string(),
                    artifact_sha256: product_proof_sha256,
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let approvals = load_provider_live_submit_approvals_for_live_node(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect("an unreferenced Hyperliquid execution client should leave approvals unselected");

        assert!(
            approvals.is_empty(),
            "a Hyperliquid client no strategy routes to must not auto-select live-submit"
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
        )
        .expect("unconsumed approval JSON should parse");
        assert_eq!(
            persisted["used_at"],
            serde_json::Value::Null,
            "a non-Hyperliquid route must not spend the Hyperliquid one-time approval artifact"
        );
    }

    fn hyperliquid_test_product_submit_proof_bytes(order_proof_path: String) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "schema_version": 1,
            "record_kind": "bolt_v3.hyperliquid_product_submit_proof.v1",
            "provider_key": "HYPERLIQUID",
            "provider_id": "hyperliquid_perps",
            "product_surface": "standard_perps",
            "toml_checksum": "b".repeat(64),
            "order_proof": {
                "artifact_path": order_proof_path,
                "artifact_sha256": "e".repeat(64),
            },
            "fill_proof": {
                "artifact_path": "operator/hyperliquid-fill-proof.json",
                "artifact_sha256": "f".repeat(64),
            },
            "rounding_proof": {
                "artifact_path": "operator/hyperliquid-rounding-proof.json",
                "artifact_sha256": "a".repeat(64),
            },
            "fee_proof": {
                "artifact_path": "operator/hyperliquid-fee-proof.json",
                "artifact_sha256": "c".repeat(64),
            },
            "settlement_proof": null,
        }))
        .expect("test product proof JSON should encode")
    }

    fn write_hyperliquid_test_product_submit_proof(path: &std::path::Path) -> String {
        let bytes = hyperliquid_test_product_submit_proof_bytes(
            "operator/hyperliquid-order-proof.json".to_string(),
        );
        std::fs::write(path, &bytes).expect("product proof should write");
        hex::encode(Sha256::digest(&bytes))
    }

    fn write_hyperliquid_semantically_invalid_product_submit_proof(
        path: &std::path::Path,
    ) -> String {
        let bytes = br#"{"provider":"HYPERLIQUID","surface":"standard_perps"}"#;
        std::fs::write(path, bytes).expect("invalid product proof should write");
        hex::encode(Sha256::digest(bytes))
    }

    #[test]
    fn live_node_invalid_product_submit_proof_schema_does_not_spend_hyperliquid_approval_artifact()
    {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
        let product_proof_sha256 =
            write_hyperliquid_semantically_invalid_product_submit_proof(&product_proof_path);
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = fixture_loaded_config_with_hyperliquid_standard_perps_route();
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                product_proof_path.display(),
                product_proof_sha256
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: product_proof_path.display().to_string(),
                    artifact_sha256: product_proof_sha256,
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let error = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect_err("matching hash alone must not authorize live-submit approval consumption");

        assert!(
            error.to_string().contains("product_submit_proof"),
            "failure should identify the product proof schema: {error}"
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
        )
        .expect("unconsumed approval JSON should parse");
        assert_eq!(
            persisted["used_at"],
            serde_json::Value::Null,
            "invalid product proof semantics must not spend one-time approval artifacts"
        );
    }

    fn write_hyperliquid_test_product_submit_proof_with_padding(
        path: &std::path::Path,
        padding_len: usize,
    ) -> String {
        let bytes = hyperliquid_test_product_submit_proof_bytes(format!(
            "operator/{}-hyperliquid-order-proof.json",
            "x".repeat(padding_len)
        ));
        std::fs::write(path, &bytes).expect("padded product proof should write");
        hex::encode(Sha256::digest(&bytes))
    }

    #[test]
    fn live_node_product_submit_proof_uses_independent_byte_cap() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
        let product_proof_sha256 =
            write_hyperliquid_test_product_submit_proof_with_padding(&product_proof_path, 6000);
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = fixture_loaded_config_with_hyperliquid_standard_perps_route();
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 4096
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 8192
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                product_proof_path.display(),
                product_proof_sha256
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: product_proof_path.display().to_string(),
                    artifact_sha256: product_proof_sha256,
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect("product proof should use its own byte cap before approval consumption");

        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("consumed approval should still read"),
        )
        .expect("consumed approval JSON should parse");
        assert_eq!(persisted["used_at"], now);
    }

    #[test]
    fn live_node_static_target_surface_mismatch_does_not_spend_hyperliquid_approval_artifact() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.strategies.truncate(1);
        let strategy = loaded
            .strategies
            .first_mut()
            .expect("fixture should include one strategy");
        strategy.config.execution_client_id = ClientId::from("hyperliquid_perps");
        strategy.config.target = toml::toml! {
            configured_target_id = "hl-spot-btc-usdc"
            kind = "static_instrument"
            rotating_market_family = "hyperliquid_instrument"
            product_surface = "spot"
            instrument_id = "BTC/USDC.HYPERLIQUID"
            quantity_step = "0.001"
        }
        .into();
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "operator/hyperliquid-product-submit-proof.json"
live_submit.standard_perps.product_proof_artifact_sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display()
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: "operator/hyperliquid-product-submit-proof.json".to_string(),
                    artifact_sha256: "d".repeat(64),
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let error = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect_err("static target surface mismatch must fail before approval consumption");

        assert!(
            error.to_string().contains("execution.product_surfaces"),
            "failure should identify the missing execution product surface: {error}"
        );
        assert!(
            error.to_string().contains("spot"),
            "failure should identify the active target surface: {error}"
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
        )
        .expect("unconsumed approval JSON should parse");
        assert_eq!(
            persisted["used_at"],
            serde_json::Value::Null,
            "surface mismatches must not spend one-time approval artifacts"
        );
    }

    #[test]
    fn live_node_missing_product_submit_proof_does_not_spend_hyperliquid_approval_artifact() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let missing_product_proof_path = temp.path().join("missing-product-submit-proof.json");
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = fixture_loaded_config_with_hyperliquid_standard_perps_route();
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                missing_product_proof_path.display()
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: missing_product_proof_path.display().to_string(),
                    artifact_sha256: "d".repeat(64),
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let error = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect_err("missing product submit proof must fail before approval consumption");

        assert!(
            error.to_string().contains("product_submit_proof"),
            "failure should identify the missing product proof binding: {error}"
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
        )
        .expect("unconsumed approval JSON should parse");
        assert_eq!(
            persisted["used_at"],
            serde_json::Value::Null,
            "missing product proof must not spend one-time approval artifacts"
        );
    }

    #[test]
    fn live_node_mismatched_product_submit_proof_does_not_spend_hyperliquid_approval_artifact() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let approval_path = temp.path().join("hyperliquid-live-submit-approval.json");
        let product_proof_path = temp.path().join("hyperliquid-product-submit-proof.json");
        let _actual_product_proof_sha256 =
            write_hyperliquid_test_product_submit_proof(&product_proof_path);
        let mismatched_product_proof_sha256 = "d".repeat(64);
        let private_key = format!("0x{}", "1".repeat(64));
        let mut loaded = fixture_loaded_config_with_hyperliquid_standard_perps_route();
        loaded.config_bundle_checksum = "b".repeat(64);
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_perps".to_string(),
            toml::from_str(&format!(
                r#"
venue = "HYPERLIQUID"

[execution]
account_id = "HYPERLIQUID-001"
environment = "testnet"
execution_mode = "master_account_api_wallet"
product_surfaces = ["standard_perps"]
live_submit.standard_perps.approval_id = "hl-standard-perps-approval-001"
live_submit.standard_perps.approval_artifact_path = "{}"
live_submit.standard_perps.approval_artifact_max_bytes = 16384
live_submit.standard_perps.max_order_count = 2
live_submit.standard_perps.max_order_notional = "25.00"
live_submit.standard_perps.product_proof_artifact_path = "{}"
live_submit.standard_perps.product_proof_artifact_sha256 = "{}"
live_submit.standard_perps.product_proof_artifact_max_bytes = 16384
base_url_ws = "wss://api.hyperliquid-testnet.xyz/ws"
base_url_http = "https://api.hyperliquid-testnet.xyz/info"
base_url_exchange = "https://api.hyperliquid-testnet.xyz/exchange"
proxy_url = "http://127.0.0.1:8080"
http_timeout_secs = 60
max_retries = 3
retry_delay_initial_ms = 250
retry_delay_max_ms = 2000
normalize_prices = true
market_order_slippage_bps = 50
transport_backend = "sockudo"
ws_post_timeout_secs = 10
outcome_settlement_poll_secs = 0

[secrets]
private_key_ssm_path = "/bolt/hyperliquid/master_api_wallet/private_key"
account_address_ssm_path = "/bolt/hyperliquid/master_api_wallet/account_address"
"#,
                approval_path.display(),
                product_proof_path.display(),
                mismatched_product_proof_sha256
            ))
            .expect("Hyperliquid client TOML should parse"),
        );
        let build_head_sha = "a".repeat(40);
        let now = 1_800_000_000;
        write_hyperliquid_live_submit_approval_artifact(
            HyperliquidLiveSubmitApprovalInput {
                approval_id: "hl-standard-perps-approval-001".to_string(),
                base_sha: build_head_sha.clone(),
                provider_id: "hyperliquid_perps".to_string(),
                product_surface:
                    crate::bolt_v3_providers::hyperliquid::HyperliquidProductSurface::StandardPerps,
                toml_checksum: loaded.config_bundle_checksum.clone(),
                signer_fingerprint: hyperliquid_live_submit_signer_fingerprint(&private_key),
                order_limits: HyperliquidLiveSubmitOrderLimits {
                    max_order_count: 2,
                    max_order_notional: "25.00".to_string(),
                },
                product_submit_proof: HyperliquidProductSubmitProofBinding {
                    artifact_path: product_proof_path.display().to_string(),
                    artifact_sha256: mismatched_product_proof_sha256,
                },
                expires_at: now + 300,
                used_at: None,
            },
            &approval_path,
        )
        .expect("approval artifact should write");
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::from([(
                "hyperliquid_perps".to_string(),
                Arc::new(ResolvedBoltV3HyperliquidSecrets {
                    private_key: Zeroizing::new(private_key),
                    account_address: Zeroizing::new(format!("0x{}", "2".repeat(40))),
                    vault_address: None,
                }) as _,
            )]),
        };

        let error = live_node_adapter_bundle_with_provider_approvals_at(
            &loaded,
            &resolved,
            now,
            &build_head_sha,
        )
        .expect_err("mismatched product submit proof must fail before approval consumption");

        assert!(
            error
                .to_string()
                .contains("product_submit_proof.artifact_sha256"),
            "failure should identify the product proof checksum: {error}"
        );
        let persisted: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&approval_path).expect("unconsumed approval should still read"),
        )
        .expect("unconsumed approval JSON should parse");
        assert_eq!(
            persisted["used_at"],
            serde_json::Value::Null,
            "mismatched product proof must not spend one-time approval artifacts"
        );
    }

    #[test]
    fn chunk_universe_splits_into_consecutive_chunks_of_at_most_n() {
        let universe: Vec<u32> = (0..10).collect();
        assert_eq!(
            chunk_universe(&universe, 3),
            vec![vec![0, 1, 2], vec![3, 4, 5], vec![6, 7, 8], vec![9]],
            "chunks must be consecutive, in order, and at most chunk_size"
        );
    }

    #[test]
    fn chunk_universe_returns_single_chunk_when_universe_fits() {
        assert_eq!(chunk_universe(&["a", "b"], 5), vec![vec!["a", "b"]]);
    }

    #[test]
    fn chunk_universe_is_empty_for_empty_universe_or_zero_chunk_size() {
        assert!(chunk_universe::<u32>(&[], 4).is_empty());
        assert!(
            chunk_universe(&[1, 2, 3], 0).is_empty(),
            "chunk_size 0 must yield no chunks so the probe fails closed rather than panicking"
        );
    }

    #[test]
    fn trade_chunk_count_probe_passes_only_at_or_above_m_with_positive_m() {
        assert!(
            !trade_chunk_count_probe_passed(0, 0),
            "m=0 must fail closed: requiring nothing proves nothing"
        );
        assert!(
            !trade_chunk_count_probe_passed(5, 0),
            "m=0 must fail closed even with fires"
        );
        assert!(!trade_chunk_count_probe_passed(9, 10), "below m must fail");
        assert!(
            trade_chunk_count_probe_passed(10, 10),
            "exactly m must pass"
        );
        assert!(trade_chunk_count_probe_passed(11, 10), "above m must pass");
    }

    fn readiness_trade_tick(instrument_id: InstrumentId, trade_id: &str) -> TradeTick {
        TradeTick::new(
            instrument_id,
            Price::from("1.00"),
            nautilus_model::types::Quantity::from("1.00"),
            AggressorSide::Buyer,
            TradeId::from(trade_id),
            1.into(),
            1.into(),
        )
    }

    #[test]
    fn chunk_count_handle_chunks_universe_and_walks_in_sorted_order() {
        let handle =
            BoltV3StrategyFreeReferenceQuoteProbeHandle::from_metadata_response_chunk_count_plan(
                ClientId::from("okx_data"),
                2,
                45,
                3,
                DataClientReadinessProbeMarketDataKind::Trade,
            );
        assert!(handle.is_chunk_count_mode());
        assert!(!handle.chunk_walk_started());

        handle.chunk_count_capture_universe(vec![
            InstrumentId::from("C-3.OKX"),
            InstrumentId::from("A-1.OKX"),
            InstrumentId::from("B-2.OKX"),
        ]);
        assert!(handle.chunk_walk_started());
        // 3 instruments at chunk_size 2 => 2 chunks; window threads through.
        assert_eq!(handle.chunk_walk_dims(), (2, 45));

        let first: Vec<String> = handle
            .chunk_count_next_chunk()
            .expect("first chunk")
            .iter()
            .map(|subscription| subscription.instrument_id.to_string())
            .collect();
        assert_eq!(
            first,
            vec!["A-1.OKX".to_string(), "B-2.OKX".to_string()],
            "the universe is walked in deterministic sorted order"
        );
        assert_eq!(
            handle.chunk_count_current_chunk().len(),
            2,
            "the current chunk tracks what is subscribed, for unsubscribe on advance"
        );

        assert_eq!(
            handle.chunk_count_next_chunk().expect("second chunk").len(),
            1,
            "the trailing chunk holds the remainder"
        );
        assert!(
            handle.chunk_count_next_chunk().is_none(),
            "the walk is exhausted after the last chunk"
        );
        assert!(
            !handle.chunk_count_passed(),
            "with no trades recorded the pass rule fails closed"
        );
    }

    #[test]
    fn chunk_count_handle_passes_after_distinct_trade_markets_reach_m() {
        let handle =
            BoltV3StrategyFreeReferenceQuoteProbeHandle::from_metadata_response_chunk_count_plan(
                ClientId::from("okx_data"),
                3,
                45,
                2,
                DataClientReadinessProbeMarketDataKind::Trade,
            );
        handle.chunk_count_capture_universe(vec![
            InstrumentId::from("A-1.OKX"),
            InstrumentId::from("B-2.OKX"),
            InstrumentId::from("C-3.OKX"),
        ]);
        let chunk = handle.chunk_count_next_chunk().expect("first chunk");

        handle.record_trade(&readiness_trade_tick(chunk[0].instrument_id, "T-A1"));
        assert!(
            !handle.has_all_required_market_data(),
            "one distinct firing market is below m=2"
        );

        handle.record_trade(&readiness_trade_tick(chunk[0].instrument_id, "T-A2"));
        assert!(
            !handle.has_all_required_market_data(),
            "duplicate trades from the same market must not double-count"
        );

        handle.record_trade(&readiness_trade_tick(chunk[1].instrument_id, "T-B1"));
        assert!(
            handle.has_all_required_market_data(),
            "the trade chunk-count probe should pass once m distinct markets fire"
        );
    }

    #[test]
    fn chunk_count_handle_fails_closed_when_universe_exhausts_below_m() {
        let handle =
            BoltV3StrategyFreeReferenceQuoteProbeHandle::from_metadata_response_chunk_count_plan(
                ClientId::from("okx_data"),
                1,
                45,
                2,
                DataClientReadinessProbeMarketDataKind::Trade,
            );
        handle.chunk_count_capture_universe(vec![InstrumentId::from("A-1.OKX")]);
        let chunk = handle.chunk_count_next_chunk().expect("first chunk");
        handle.record_trade(&readiness_trade_tick(chunk[0].instrument_id, "T-A1"));

        assert!(
            handle.chunk_count_next_chunk().is_none(),
            "the single-market universe is exhausted after one chunk"
        );
        let failure = handle
            .failure_error()
            .expect("exhausting below m must set a fail-closed reason");
        assert!(
            failure.contains("below required min_observed_targets=2"),
            "failure should explain the unmet m threshold: {failure}"
        );
        assert!(
            !handle.has_all_required_market_data(),
            "exhaustion below m must never satisfy readiness"
        );
    }

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        let root_text = include_str!("../tests/fixtures/bolt_v3/root.toml");
        let mut root: BoltV3RootConfig = toml::from_str(root_text).unwrap();
        let catalog_id = NEXT_TEST_CATALOG_ID.fetch_add(1, Ordering::Relaxed);
        root.persistence.catalog_directory = std::env::temp_dir()
            .join(format!(
                "bolt-v3-live-node-test-catalog-{}-{catalog_id}",
                std::process::id()
            ))
            .to_string_lossy()
            .to_string();
        LoadedBoltV3Config {
            root_path: std::path::PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            config_bundle_checksum: String::new(),
            root,
            strategies: Vec::new(),
        }
    }

    fn fixture_loaded_config_with_hyperliquid_standard_perps_route() -> LoadedBoltV3Config {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded.strategies.truncate(1);
        let strategy = loaded
            .strategies
            .first_mut()
            .expect("fixture should include one strategy");
        strategy.config.execution_client_id = ClientId::from("hyperliquid_perps");
        strategy.config.target = toml::toml! {
            configured_target_id = "hl-standard-perps-btc"
            kind = "static_instrument"
            rotating_market_family = "hyperliquid_instrument"
            product_surface = "standard_perps"
            instrument_id = "BTC-PERP.HYPERLIQUID"
            quantity_step = "0.001"
        }
        .into();
        loaded
    }

    fn write_venue_spendability_source(
        loaded: &mut LoadedBoltV3Config,
        temp_path: &std::path::Path,
        observed_at_ns: u64,
        spendable_collateral: &str,
        collateral_allowance: &str,
    ) {
        let path = temp_path.join("venue-spendability-source.json");
        let pool = loaded
            .root
            .risk
            .capital_pools
            .as_mut()
            .and_then(|pools| pools.first_mut())
            .expect("fixture should configure a capital pool");
        pool.enforce_submit_admission = true;
        let payload = format!(
            r#"{{
  "schema_version": {schema_version},
  "record_kind": "{record_kind}",
  "source": "operator_venue_spendability",
  "observed_at_ns": {observed_at_ns},
  "venue_id": "{venue_id}",
  "account_id": "{account_id}",
  "collateral_currency": "{collateral_currency}",
  "spendable_collateral": "{spendable_collateral}",
  "collateral_allowance": "{collateral_allowance}"
}}"#,
            schema_version = crate::bolt_v3_sizing_state::VENUE_SPENDABILITY_SOURCE_SCHEMA_VERSION,
            record_kind = crate::bolt_v3_sizing_state::VENUE_SPENDABILITY_SOURCE_RECORD_KIND,
            venue_id = pool.venue_id,
            account_id = pool.account_id,
            collateral_currency = pool.collateral_currency,
        );
        std::fs::write(&path, payload.as_bytes()).expect("spendability source should write");
        pool.venue_spendability_source_path = Some(path.to_string_lossy().to_string());
        pool.venue_spendability_source_sha256 =
            Some(hex::encode(Sha256::digest(payload.as_bytes())));
        pool.venue_spendability_source_max_bytes = Some(16_384);
    }

    fn insert_configured_data_client(loaded: &mut LoadedBoltV3Config) {
        loaded.root.clients.insert(
            "configured-client".to_string(),
            toml::from_str(
                r#"
venue = "OKX"

[data]
configured_data_param = "configured-value"
"#,
            )
            .expect("configured data client should parse"),
        );
    }

    fn test_data_client(venue: &str) -> ClientBlock {
        ClientBlock {
            venue: Venue::from(venue),
            data: Some(toml::Value::Table(toml::map::Map::new())),
            execution: None,
            secrets: None,
            readiness_probe: None,
        }
    }

    fn test_execution_client(venue: &str) -> ClientBlock {
        ClientBlock {
            venue: Venue::from(venue),
            data: None,
            execution: Some(toml::Value::Table(toml::map::Map::new())),
            secrets: None,
            readiness_probe: None,
        }
    }

    fn test_rv_source(
        source_id: &str,
        client_id: &str,
        instrument_id: &str,
        enabled: bool,
    ) -> RealizedVolatilitySourceBlock {
        RealizedVolatilitySourceBlock {
            source_id: source_id.to_string(),
            data_client_id: ClientId::from(client_id),
            instrument_id: InstrumentId::from(instrument_id),
            source_class: RealizedVolatilitySourceClassBlock::SpotQuote,
            sample_kind: RealizedVolatilitySampleKindBlock::Midpoint,
            enabled,
            counts_toward_quorum: enabled,
            canonical_quote_asset: "USDT".to_string(),
        }
    }

    fn test_rv_surface(
        sources: Vec<RealizedVolatilitySourceBlock>,
    ) -> RealizedVolatilitySurfaceBlock {
        RealizedVolatilitySurfaceBlock {
            canonical_base_asset: "CONFIGURED_ASSET".to_string(),
            canonical_quote_asset: "USDT".to_string(),
            policy: RealizedVolatilityPolicyBlock {
                window_ms: 600_000,
                sampling_interval_ms: 1_000,
                min_ready_sources: 1,
                max_source_age_ms: 60_000,
                max_event_receive_lag_ms: 1_000,
                max_inter_sample_gap_ms: 60_000,
                min_coverage_ratio: 0.5,
                max_cross_source_dispersion: 1.0,
                seconds_per_annum: 31_536_000.0,
                aggregation: RealizedVolatilityAggregationBlock::UpperQuantile,
                upper_quantile: 1.0,
                trim_fraction: None,
                guard_weight: None,
            },
            estimator: None,
            sources,
        }
    }

    fn insert_test_rv_surface(
        loaded: &mut LoadedBoltV3Config,
        surface_id: &str,
        sources: Vec<RealizedVolatilitySourceBlock>,
    ) {
        loaded
            .root
            .realized_volatility_surfaces
            .get_or_insert_with(BTreeMap::new)
            .insert(surface_id.to_string(), test_rv_surface(sources));
    }

    fn loaded_config_with_rv_only_source() -> LoadedBoltV3Config {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded
            .root
            .clients
            .insert("rv_only_data".to_string(), test_data_client("OKX"));
        insert_test_rv_surface(
            &mut loaded,
            "rv_only_surface",
            vec![test_rv_source(
                "rv_only_midpoint",
                "rv_only_data",
                "CONFIGURED_ASSET-USDT-RVONLY.OKX",
                true,
            )],
        );
        loaded
    }

    fn test_registration_controls(
        writer: Arc<dyn BoltV3DecisionEvidenceWriter>,
    ) -> BoltV3StrategyExecutionControls {
        BoltV3StrategyExecutionControls {
            submit_admission: Arc::new(BoltV3SubmitAdmissionState::new(writer)),
            order_execution_policy:
                crate::bolt_v3_order_execution::BoltV3OrderExecutionPolicy::live(),
        }
    }

    fn fixture_loaded_config_with_external_option_greeks_iv() -> LoadedBoltV3Config {
        let mut loaded = fixture_loaded_config();
        loaded.root.clients.clear();
        insert_configured_data_client(&mut loaded);
        loaded.root.nautilus.data_engine.external_clients =
            vec![ClientId::from("configured-client")];
        loaded.root.iv = Some(
            toml::from_str(
                r#"
schema_version = 1

[[profiles]]
profile_id = "configured-profile"
enabled_products = ["source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
max_source_event_future_skew_ns = 0
input_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv", "ConfiguredNtSymbol"] } }
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[profiles.audit_policy]
profile_id = "configured-profile"
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["BTC-20240101-50000-C.DERIBIT"]

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"
"#,
            )
            .expect("configured IV profile should parse"),
        );
        loaded
    }

    #[test]
    fn live_node_startup_applies_iv_subscription_plans_to_runtime_source_health() {
        let mut loaded = fixture_loaded_config();
        loaded.root.clients.clear();
        insert_configured_data_client(&mut loaded);
        loaded.root.nautilus.data_engine.external_clients =
            vec![ClientId::from("configured-client")];
        loaded.root.iv = Some(
            toml::from_str(
                r#"
schema_version = 1

[[profiles]]
profile_id = "configured-profile"
enabled_products = ["source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
max_source_event_future_skew_ns = 0
input_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv", "ConfiguredNtSymbol"] } }
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[profiles.audit_policy]
profile_id = "configured-profile"
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "configured-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["BTC-20240101-50000-C.DERIBIT"]

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"
"#,
            )
            .expect("configured IV profile should parse"),
        );
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let adapters = BoltV3AdapterConfigs {
            clients: BTreeMap::new(),
        };

        let (runtime, _) = build_live_node_with_clients_and_submit_approval_limits(
            &loaded,
            &resolved,
            adapters,
            BTreeMap::new(),
        )
        .expect("configured external IV source should build without live transport");

        assert!(runtime.has_iv_runtime());
        let health = runtime
            .iv_source_health("configured-profile", "configured-greeks-source")
            .expect("startup should apply IV source health");
        assert_eq!(
            health.subscription_state,
            crate::bolt_v3_iv::health::IvSourceHealthState::Subscribing
        );
        assert_eq!(health.subscription_generation, 7);
    }

    #[test]
    fn live_node_startup_rejects_unknown_iv_data_client() {
        let mut loaded = fixture_loaded_config();
        loaded.root.clients.clear();
        loaded.root.nautilus.data_engine.external_clients.clear();
        loaded.root.iv = Some(
            toml::from_str(
                r#"
schema_version = 1

[[profiles]]
profile_id = "configured-profile"
enabled_products = ["source_health"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
max_source_event_future_skew_ns = 0
input_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv", "ConfiguredNtSymbol"] } }
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[profiles.audit_policy]
profile_id = "configured-profile"
enabled_raw_products = ["option_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-greeks-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["source_health"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "configured-greeks-source"
selector_fingerprint = "configured-greeks-selector"
source_kind = "option_greeks"
client_id = "missing-client"
subscription_generation = 7
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredOptionGreeks"

[profiles.sources.selector]
selector_kind = "source_option_greeks"
instrument_ids = ["BTC-20240101-50000-C.DERIBIT"]

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"
"#,
            )
            .expect("configured IV profile should parse"),
        );
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let adapters = BoltV3AdapterConfigs {
            clients: BTreeMap::new(),
        };

        let error = build_live_node_with_clients_and_submit_approval_limits(
            &loaded,
            &resolved,
            adapters,
            BTreeMap::new(),
        )
        .expect_err("unknown IV source client must reject before live-node build");

        assert!(format!("{error:?}").contains("missing-client"));
    }

    #[test]
    fn iv_option_greeks_identifier_list_rejects_before_runtime_commands() {
        let ids = vec![
            "BTC-20240101-50000-C.DERIBIT".to_string(),
            "configured-invalid-option-instrument".to_string(),
        ];

        let error = parse_option_greeks_instrument_ids(&ids).expect_err("invalid ID should reject");

        assert!(error.contains("invalid NT option-greeks instrument_id"));
        assert!(error.contains("configured-invalid-option-instrument"));
    }

    #[test]
    fn iv_option_chain_identifier_list_rejects_before_runtime_commands() {
        let ids = vec![
            "DERIBIT:BTC:BTC:2024-01-01".to_string(),
            "configured-invalid-option-series".to_string(),
        ];

        let error = parse_option_chain_series_ids(&ids).expect_err("invalid ID should reject");

        assert!(error.contains("invalid NT option-chain series_id"));
        assert!(error.contains("configured-invalid-option-series"));
    }

    #[test]
    fn iv_option_greeks_start_plan_translates_to_runtime_data_command() {
        let plan = IvSubscriptionPlan {
            profile_id: "configured-profile".to_string(),
            source_id: "configured-greeks-source".to_string(),
            lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeOptionGreeks,
            nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionGreeks,
            client_id: "configured-client".to_string(),
            selector: IvSelector::SourceOptionGreeks {
                instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
                nt_params: toml::Value::Table(toml::map::Map::new()),
            },
            params: toml::Value::Table(toml::map::Map::new()),
            subscription_generation: 7,
        };

        let commands = iv_runtime_data_commands_for_plan(&plan)
            .expect("valid option-greeks plan should translate to an NT data command");

        assert_eq!(commands.len(), 1);
        match &commands[0] {
            nautilus_common::messages::data::DataCommand::Subscribe(
                SubscribeCommand::OptionGreeks(command),
            ) => {
                assert_eq!(
                    command.instrument_id,
                    InstrumentId::from("BTC-20240101-50000-C.DERIBIT")
                );
                assert_eq!(command.client_id, Some(ClientId::from("configured-client")));
            }
            other => panic!("expected option-greeks subscribe command, got {other:?}"),
        }
    }

    #[test]
    fn iv_remove_source_plan_translates_to_no_runtime_data_commands() {
        let plan = IvSubscriptionPlan {
            profile_id: "configured-profile".to_string(),
            source_id: "configured-greeks-source".to_string(),
            lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::SourceRemoval,
            operation: IvRuntimeOperation::RemoveSource,
            nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionGreeks,
            client_id: "configured-client".to_string(),
            selector: IvSelector::SourceOptionGreeks {
                instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
                nt_params: toml::Value::Table(toml::map::Map::new()),
            },
            params: toml::Value::Table(toml::map::Map::new()),
            subscription_generation: 7,
        };

        let commands = iv_runtime_data_commands_for_plan(&plan)
            .expect("source removal should not require NT data commands");

        assert!(commands.is_empty());
    }

    #[test]
    fn iv_option_chain_start_plan_translates_parseable_strike_range_to_runtime_data_command() {
        let plan = IvSubscriptionPlan {
            profile_id: "configured-profile".to_string(),
            source_id: "configured-chain-source".to_string(),
            lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeOptionChain,
            nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionChain,
            client_id: "configured-client".to_string(),
            selector: IvSelector::SourceOptionChain {
                series_ids: vec!["DERIBIT:BTC:BTC:2024-01-01T00:00:00Z".to_string()],
                strike_range_policy: "atm_relative:1:1".to_string(),
                nt_params: toml::toml! {
                    snapshot_interval_ms = 250
                }
                .into(),
            },
            params: toml::Value::Table(toml::map::Map::new()),
            subscription_generation: 7,
        };

        let commands = iv_runtime_data_commands_for_plan(&plan)
            .expect("valid option-chain plan should translate to an NT data command");

        assert_eq!(commands.len(), 1);
        match &commands[0] {
            nautilus_common::messages::data::DataCommand::Subscribe(
                SubscribeCommand::OptionChain(command),
            ) => {
                assert_eq!(
                    command.series_id,
                    OptionSeriesId::from_str("DERIBIT:BTC:BTC:2024-01-01T00:00:00Z").unwrap()
                );
                assert_eq!(
                    command.strike_range,
                    StrikeRange::AtmRelative {
                        strikes_above: 1,
                        strikes_below: 1,
                    }
                );
                assert_eq!(command.snapshot_interval_ms, Some(250));
                assert_eq!(command.client_id, Some(ClientId::from("configured-client")));
            }
            other => panic!("expected option-chain subscribe command, got {other:?}"),
        }
    }

    #[test]
    fn iv_custom_iv_start_plan_translates_to_runtime_custom_data_command() {
        let plan = IvSubscriptionPlan {
            profile_id: "configured-profile".to_string(),
            source_id: "configured-custom-source".to_string(),
            lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeCustomData,
            nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::CustomData,
            client_id: "configured-client".to_string(),
            selector: IvSelector::SourceCustomImpliedVolatility {
                custom_iv_data_type: "ConfiguredCustomIvEvent".to_string(),
                custom_iv_data_fields: vec!["configured_iv".to_string()],
                nt_params: toml::toml! {
                    configured_selector_param = "selector-value"
                }
                .into(),
            },
            params: toml::toml! {
                configured_source_param = "source-value"
            }
            .into(),
            subscription_generation: 7,
        };

        let commands = iv_runtime_data_commands_for_plan(&plan)
            .expect("valid custom-IV plan should translate to an NT custom-data command");

        assert_eq!(commands.len(), 1);
        match &commands[0] {
            nautilus_common::messages::data::DataCommand::Subscribe(SubscribeCommand::Data(
                command,
            )) => {
                assert_eq!(command.client_id, Some(ClientId::from("configured-client")));
                assert_eq!(command.data_type.type_name(), "ConfiguredCustomIvEvent");
                assert_eq!(
                    command.data_type.identifier(),
                    Some("configured-custom-source")
                );
                let metadata = command
                    .data_type
                    .metadata()
                    .expect("custom-IV data type should carry merged params");
                assert_eq!(
                    metadata.get("configured_source_param"),
                    Some(&serde_json::Value::String("source-value".to_string()))
                );
                assert_eq!(
                    metadata.get("configured_selector_param"),
                    Some(&serde_json::Value::String("selector-value".to_string()))
                );
                assert_eq!(command.params.as_ref(), Some(metadata));
            }
            other => panic!("expected custom-IV data subscribe command, got {other:?}"),
        }
    }

    #[test]
    fn iv_aggregate_greeks_start_plan_translates_to_runtime_custom_data_command() {
        let plan = IvSubscriptionPlan {
            profile_id: "configured-profile".to_string(),
            source_id: "configured-aggregate-source".to_string(),
            lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeAggregateGreeks,
            nt_source_kind:
                crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::AggregateGreeksTopic,
            client_id: "configured-client".to_string(),
            selector: IvSelector::SourceAggregateGreeks {
                aggregate_key: "ConfiguredAggregateGreeksEvent".to_string(),
                underlying_selectors: vec!["configured-underlying-selector".to_string()],
                delta_field: "configured_delta".to_string(),
                gamma_field: "configured_gamma".to_string(),
                vega_field: "configured_vega".to_string(),
                theta_field: "configured_theta".to_string(),
                rho_field: "configured_rho".to_string(),
                iv_field: Some("configured_iv".to_string()),
                iv_basis: None,
                iv_convention: None,
                nt_params: toml::toml! {
                    configured_selector_param = "selector-value"
                }
                .into(),
            },
            params: toml::toml! {
                configured_source_param = "source-value"
            }
            .into(),
            subscription_generation: 7,
        };

        let commands = iv_runtime_data_commands_for_plan(&plan)
            .expect("valid aggregate-greeks plan should translate to an NT custom-data command");

        assert_eq!(commands.len(), 1);
        match &commands[0] {
            nautilus_common::messages::data::DataCommand::Subscribe(SubscribeCommand::Data(
                command,
            )) => {
                assert_eq!(command.client_id, Some(ClientId::from("configured-client")));
                assert_eq!(
                    command.data_type.type_name(),
                    "ConfiguredAggregateGreeksEvent"
                );
                assert_eq!(
                    command.data_type.identifier(),
                    Some("configured-aggregate-source")
                );
                let metadata = command
                    .data_type
                    .metadata()
                    .expect("aggregate-greeks data type should carry merged params");
                assert_eq!(
                    metadata.get("underlying_selectors"),
                    Some(&serde_json::Value::Array(vec![serde_json::Value::String(
                        "configured-underlying-selector".to_string()
                    )]))
                );
                assert_eq!(
                    metadata.get("configured_source_param"),
                    Some(&serde_json::Value::String("source-value".to_string()))
                );
                assert_eq!(
                    metadata.get("configured_selector_param"),
                    Some(&serde_json::Value::String("selector-value".to_string()))
                );
                assert_eq!(command.params.as_ref(), Some(metadata));
            }
            other => panic!("expected aggregate-greeks data subscribe command, got {other:?}"),
        }
    }

    #[derive(Debug)]
    struct RecordingDataCommandSender {
        commands: std::sync::Arc<std::sync::Mutex<Vec<DataCommand>>>,
    }

    impl nautilus_common::runner::DataCommandSender for RecordingDataCommandSender {
        fn execute(&self, command: DataCommand) {
            self.commands
                .lock()
                .expect("recording data command sender lock should not be poisoned")
                .push(command);
        }
    }

    struct DataCommandSenderRestore;

    impl Drop for DataCommandSenderRestore {
        fn drop(&mut self) {
            nautilus_common::runner::replace_data_cmd_sender(std::sync::Arc::new(
                nautilus_common::runner::SyncDataCommandSender,
            ));
        }
    }

    #[test]
    fn iv_runtime_command_sender_adapter_queues_start_plan_after_runner_sender_is_bound() {
        let commands = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        nautilus_common::runner::replace_data_cmd_sender(std::sync::Arc::new(
            RecordingDataCommandSender {
                commands: commands.clone(),
            },
        ));
        let _restore_sender = DataCommandSenderRestore;
        let plan = IvSubscriptionPlan {
            profile_id: "configured-profile".to_string(),
            source_id: "configured-greeks-source".to_string(),
            lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeOptionGreeks,
            nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionGreeks,
            client_id: "configured-client".to_string(),
            selector: IvSelector::SourceOptionGreeks {
                instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
                nt_params: toml::Value::Table(toml::map::Map::new()),
            },
            params: toml::Value::Table(toml::map::Map::new()),
            subscription_generation: 7,
        };
        let mut adapter =
            NtIvRuntimeCommandSenderAdapter::new(&[ClientId::from("configured-client")], &[]);

        adapter
            .apply_subscription_plan(&plan)
            .expect("valid runtime start plan should be queued");

        let commands = commands
            .lock()
            .expect("recording data command sender lock should not be poisoned");
        assert_eq!(commands.len(), 1);
        assert!(matches!(
            &commands[0],
            DataCommand::Subscribe(SubscribeCommand::OptionGreeks(_))
        ));
    }

    #[test]
    fn iv_runtime_command_sender_adapter_rejects_unknown_start_client_without_queueing() {
        let commands = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        nautilus_common::runner::replace_data_cmd_sender(std::sync::Arc::new(
            RecordingDataCommandSender {
                commands: commands.clone(),
            },
        ));
        let _restore_sender = DataCommandSenderRestore;
        let plan = IvSubscriptionPlan {
            profile_id: "configured-profile".to_string(),
            source_id: "configured-greeks-source".to_string(),
            lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeOptionGreeks,
            nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionGreeks,
            client_id: "missing-client".to_string(),
            selector: IvSelector::SourceOptionGreeks {
                instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
                nt_params: toml::Value::Table(toml::map::Map::new()),
            },
            params: toml::Value::Table(toml::map::Map::new()),
            subscription_generation: 7,
        };
        let mut adapter = NtIvRuntimeCommandSenderAdapter::new(&[], &[]);

        let error = adapter
            .apply_subscription_plan(&plan)
            .expect_err("unknown runtime start client should reject before queueing");

        assert_eq!(error.reason, IvRejectReason::SubscriptionFailed);
        assert!(error.message.contains("not registered"));
        assert!(
            commands
                .lock()
                .expect("recording data command sender lock should not be poisoned")
                .is_empty(),
            "invalid start client must not enqueue a data command"
        );
    }

    #[test]
    fn iv_runtime_command_sender_adapter_skips_external_start_client_without_queueing() {
        let commands = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        nautilus_common::runner::replace_data_cmd_sender(std::sync::Arc::new(
            RecordingDataCommandSender {
                commands: commands.clone(),
            },
        ));
        let _restore_sender = DataCommandSenderRestore;
        let plan = IvSubscriptionPlan {
            profile_id: "configured-profile".to_string(),
            source_id: "configured-greeks-source".to_string(),
            lifecycle: crate::bolt_v3_iv::subscription::IvSubscriptionLifecycle::Start,
            operation: IvRuntimeOperation::SubscribeOptionGreeks,
            nt_source_kind: crate::bolt_v3_iv::subscription::IvNtSubscriptionKind::OptionGreeks,
            client_id: "configured-client".to_string(),
            selector: IvSelector::SourceOptionGreeks {
                instrument_ids: vec!["BTC-20240101-50000-C.DERIBIT".to_string()],
                nt_params: toml::Value::Table(toml::map::Map::new()),
            },
            params: toml::Value::Table(toml::map::Map::new()),
            subscription_generation: 7,
        };
        let mut adapter =
            NtIvRuntimeCommandSenderAdapter::new(&[], &[ClientId::from("configured-client")]);

        adapter
            .apply_subscription_plan(&plan)
            .expect("external start client should be accepted without NT queueing");

        assert!(
            commands
                .lock()
                .expect("recording data command sender lock should not be poisoned")
                .is_empty(),
            "external start client is managed outside the runtime sender"
        );
    }

    #[test]
    fn live_node_runtime_stop_applies_iv_unsubscribe_lifecycle() {
        let loaded = fixture_loaded_config_with_external_option_greeks_iv();
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let adapters = BoltV3AdapterConfigs {
            clients: BTreeMap::new(),
        };

        let (mut runtime, _) = build_live_node_with_clients_and_submit_approval_limits(
            &loaded,
            &resolved,
            adapters,
            BTreeMap::new(),
        )
        .expect("configured external IV source should build without live transport");
        assert!(
            runtime.has_iv_event_bindings(),
            "startup should install IV receive-side bindings"
        );

        runtime
            .stop_iv_engine_lifecycle(&loaded.root)
            .expect("IV stop lifecycle should apply unsubscribe plans");
        assert!(
            !runtime.has_iv_event_bindings(),
            "stop should drop IV receive-side bindings"
        );
        assert!(
            !runtime.has_iv_runtime(),
            "stop should clear the IV runtime after applying unsubscribe outcomes"
        );
        assert!(
            runtime
                .iv_source_health("configured-profile", "configured-greeks-source")
                .is_none(),
            "stopped live node should not expose IV source health through a retained runtime"
        );
    }

    #[test]
    fn live_node_runtime_stop_planning_failure_keeps_iv_runtime() {
        let loaded = fixture_loaded_config_with_external_option_greeks_iv();
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let adapters = BoltV3AdapterConfigs {
            clients: BTreeMap::new(),
        };

        let (mut runtime, _) = build_live_node_with_clients_and_submit_approval_limits(
            &loaded,
            &resolved,
            adapters,
            BTreeMap::new(),
        )
        .expect("configured external IV source should build without live transport");
        assert!(runtime.has_iv_runtime());
        assert!(runtime.has_iv_event_bindings());

        let mut invalid_stop_root = loaded.root.clone();
        let profile = invalid_stop_root
            .iv
            .as_mut()
            .and_then(|iv| iv.profiles.first_mut())
            .expect("fixture should include one IV profile");
        let duplicate_source = profile
            .sources
            .first()
            .expect("fixture should include one IV source")
            .clone();
        profile.sources.push(duplicate_source);

        let error = runtime
            .stop_iv_engine_lifecycle(&invalid_stop_root)
            .expect_err("invalid stop lifecycle planning should fail");

        assert!(
            error.to_string().contains("DuplicateSourceId"),
            "failure should identify duplicate-source stop planning: {error}"
        );
        assert!(
            runtime.has_iv_runtime(),
            "failed stop planning must not drop the IV runtime"
        );
        assert!(
            runtime.has_iv_event_bindings(),
            "failed stop planning must not drop IV event bindings"
        );
    }

    #[test]
    fn live_node_startup_binds_aggregate_greeks_sources_through_nt_custom_data() {
        let mut loaded = fixture_loaded_config();
        loaded.root.clients.clear();
        insert_configured_data_client(&mut loaded);
        loaded.root.nautilus.data_engine.external_clients =
            vec![ClientId::from("configured-client")];
        loaded.root.iv = Some(
            toml::from_str(
                r#"
schema_version = 1

[[profiles]]
profile_id = "configured-profile"
enabled_products = ["source_health", "aggregate_greeks"]
max_raw_events = 2
max_indexed_points = 2
max_smiles = 2
max_surfaces = 2
max_derived_points = 2
max_source_health_events = 2
max_source_event_future_skew_ns = 0
input_bounds = { finite_required = true, positive_required = true, inclusive_min = 0.0, inclusive_max = 5.0, unit = "unitless", allowed_conventions = { allowed_conventions = ["configured-convention", "BLACK_SCHOLES", "ConfiguredOptionGreeks", "ConfiguredOptionChain", "ConfiguredAggregateGreeks", "ConfiguredCustomIv", "ConfiguredNtSymbol"] } }
projection_policies = []
interpolation_policies = []
fallback_policies = []
quorum_policies = []
helper_policies = []
derived_inputs = []
derived_input_policies = []

[profiles.audit_policy]
profile_id = "configured-profile"
enabled_raw_products = ["aggregate_greeks"]
authorized_audit_handles = ["configured-audit-handle"]
access_purposes = ["configured-replay-purpose"]
eligible_sources = ["configured-aggregate-source"]

[profiles.audit_policy.audit_retention]
max_events = 2
max_age_ns = 10000

[[profiles.strategy_authorizations]]
strategy_id = "configured-strategy"
authorization_mode = "profile_wide"
allowed_product_kinds = ["source_health", "aggregate_greeks"]
allowed_selector_fingerprints = []
allowed_source_ids = []

[[profiles.sources]]
source_id = "configured-aggregate-source"
selector_fingerprint = "configured-aggregate-selector"
source_kind = "aggregate_greeks"
client_id = "configured-client"
subscription_generation = 11
accepted_conventions = ["configured-convention"]

[profiles.sources.nt_provenance]
nt_revision = "configured-nt-revision"
nt_evidence_path = "configured/nt/evidence/path.rs"
nt_symbol = "ConfiguredAggregateGreeks"

[profiles.sources.selector]
selector_kind = "source_aggregate_greeks"
aggregate_key = "configured-aggregate-greeks-topic"
underlying_selectors = ["configured-underlying-selector"]
delta_field = "configured-delta-field"
gamma_field = "configured-gamma-field"
vega_field = "configured-vega-field"
theta_field = "configured-theta-field"
rho_field = "configured-rho-field"

[profiles.sources.selector.nt_params]
configured_nt_param = "configured-value"

[profiles.sources.params]
configured_source_param = "configured-value"
"#,
            )
            .expect("configured aggregate IV profile should parse"),
        );
        let resolved = ResolvedBoltV3Secrets {
            clients: BTreeMap::new(),
        };
        let adapters = BoltV3AdapterConfigs {
            clients: BTreeMap::new(),
        };

        let (runtime, _) = build_live_node_with_clients_and_submit_approval_limits(
            &loaded,
            &resolved,
            adapters,
            BTreeMap::new(),
        )
        .expect("configured aggregate IV source should build without live transport");

        let health = runtime
            .iv_source_health("configured-profile", "configured-aggregate-source")
            .expect("startup should apply aggregate IV source health");
        assert_eq!(
            health.subscription_state,
            crate::bolt_v3_iv::health::IvSourceHealthState::Subscribing
        );
        assert_eq!(health.subscription_generation, 11);
    }

    #[test]
    fn data_client_readiness_quote_plan_uses_client_owned_probe_targets() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include polymarket client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
            book_type: None,
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::Configured,
            max_metadata_quote_targets: None,
            allow_metadata_target_sampling: None,
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: Some(BTreeMap::from([(
                "configured_quote_probe".to_string(),
                DataClientReadinessProbeQuoteTargetBlock {
                    instrument_id: InstrumentId::from("REFERENCE.POLYMARKET"),
                },
            )])),
        });

        let (required, ambiguous) =
            strategy_free_data_client_readiness_quote_subscription_plan(&loaded, "polymarket_main")
                .expect("client-owned readiness quote plan should build");

        assert!(ambiguous.is_empty());
        assert_eq!(required.len(), 1);
        assert_eq!(
            required[0].data_client_id,
            ClientId::from("polymarket_main")
        );
        assert_eq!(
            required[0].instrument_id,
            InstrumentId::from("REFERENCE.POLYMARKET")
        );
    }

    #[test]
    fn data_client_readiness_metadata_response_probe_starts_pending_until_targets_arrive() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
            book_type: None,
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(2),
            allow_metadata_target_sampling: Some(false),
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness quote handle should build");

        assert!(
            !handle.has_all_required_quotes(),
            "metadata-response quote probes must not pass before same-run metadata installs targets"
        );
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-FIRST.SOURCE"),
            InstrumentId::from("CONFIGURED-SECOND.SOURCE"),
        ]);

        assert_eq!(installed.len(), 2);
        assert!(
            !handle.has_all_required_quotes(),
            "installing targets should not pass the quote probe until quotes arrive"
        );
        for subscription in installed {
            handle
                .quotes
                .borrow_mut()
                .push(BoltV3StrategyFreeReferenceQuote {
                    data_client_id: subscription.data_client_id.to_string(),
                    instrument_id: subscription.instrument_id.to_string(),
                    bid_price: 1.0,
                    ask_price: 2.0,
                    ts_event_unix_nanos: 1_000,
                    ts_init_unix_nanos: 1_100,
                    captured_at_unix_nanos: 1_200,
                });
        }

        assert!(
            handle.has_all_required_quotes(),
            "metadata-response quote probes should pass after every installed source-owned target has a quote"
        );
    }

    #[test]
    fn data_client_readiness_metadata_response_probe_rejects_unbounded_metadata_universe() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
            book_type: None,
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(2),
            allow_metadata_target_sampling: Some(false),
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness quote handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-FIRST.SOURCE"),
            InstrumentId::from("CONFIGURED-SECOND.SOURCE"),
            InstrumentId::from("CONFIGURED-THIRD.SOURCE"),
        ]);

        assert!(
            installed.is_empty(),
            "metadata-response probes must not truncate a broad metadata universe into an arbitrary sample"
        );
        let failure = handle
            .failure_error()
            .expect("unbounded metadata universe should fail closed");
        assert!(
            failure.contains("max_metadata_quote_targets"),
            "failure should name the TOML-owned bound: {failure}"
        );
    }

    #[test]
    fn data_client_readiness_metadata_response_probe_samples_when_explicitly_configured() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
            book_type: None,
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(3),
            allow_metadata_target_sampling: Some(true),
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness quote handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-C.SOURCE"),
            InstrumentId::from("CONFIGURED-A.SOURCE"),
            InstrumentId::from("CONFIGURED-E.SOURCE"),
            InstrumentId::from("CONFIGURED-B.SOURCE"),
            InstrumentId::from("CONFIGURED-D.SOURCE"),
        ]);

        assert_eq!(installed.len(), 3);
        assert_eq!(
            installed[0].instrument_id,
            InstrumentId::from("CONFIGURED-A.SOURCE")
        );
        assert_eq!(
            installed[1].instrument_id,
            InstrumentId::from("CONFIGURED-C.SOURCE")
        );
        assert_eq!(
            installed[2].instrument_id,
            InstrumentId::from("CONFIGURED-E.SOURCE")
        );
        assert!(handle.failure_error().is_none());
    }

    #[test]
    fn data_client_readiness_metadata_response_probe_requires_all_metadata_quote_targets() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
            book_type: None,
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(3),
            allow_metadata_target_sampling: Some(false),
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness quote handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-FIRST.SOURCE"),
            InstrumentId::from("CONFIGURED-SECOND.SOURCE"),
            InstrumentId::from("CONFIGURED-THIRD.SOURCE"),
        ]);

        for subscription in installed.iter().take(1) {
            handle
                .quotes
                .borrow_mut()
                .push(BoltV3StrategyFreeReferenceQuote {
                    data_client_id: subscription.data_client_id.to_string(),
                    instrument_id: subscription.instrument_id.to_string(),
                    bid_price: 1.0,
                    ask_price: 2.0,
                    ts_event_unix_nanos: 1_000,
                    ts_init_unix_nanos: 1_100,
                    captured_at_unix_nanos: 1_200,
                });
        }
        assert!(
            !handle.has_all_required_quotes(),
            "metadata-response quote probe must not pass before every same-run metadata target is observed"
        );

        let subscription = installed
            .get(1)
            .expect("second source-owned target should be installed");
        handle
            .quotes
            .borrow_mut()
            .push(BoltV3StrategyFreeReferenceQuote {
                data_client_id: subscription.data_client_id.to_string(),
                instrument_id: subscription.instrument_id.to_string(),
                bid_price: 1.0,
                ask_price: 2.0,
                ts_event_unix_nanos: 1_000,
                ts_init_unix_nanos: 1_100,
                captured_at_unix_nanos: 1_200,
            });

        assert!(
            !handle.has_all_required_quotes(),
            "metadata-response quote probe should still wait for the final same-run metadata target"
        );

        let subscription = installed
            .get(2)
            .expect("third source-owned target should be installed");
        handle
            .quotes
            .borrow_mut()
            .push(BoltV3StrategyFreeReferenceQuote {
                data_client_id: subscription.data_client_id.to_string(),
                instrument_id: subscription.instrument_id.to_string(),
                bid_price: 1.0,
                ask_price: 2.0,
                ts_event_unix_nanos: 1_000,
                ts_init_unix_nanos: 1_100,
                captured_at_unix_nanos: 1_200,
            });

        assert!(
            handle.has_all_required_quotes(),
            "metadata-response quote probe should pass after all same-run metadata targets are observed"
        );
    }

    #[test]
    fn data_client_readiness_metadata_response_probe_accepts_book_deltas_when_configured() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
            book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(1),
            allow_metadata_target_sampling: Some(false),
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness book handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![InstrumentId::from(
            "CONFIGURED-FIRST.SOURCE",
        )]);

        assert_eq!(installed.len(), 1);
        assert!(
            !handle.has_all_required_market_data(),
            "book probes must not pass before a source-owned book-delta event arrives"
        );
        let subscription = &installed[0];
        let delta = OrderBookDelta::new(
            subscription.instrument_id,
            BookAction::Add,
            BookOrder::new(
                OrderSide::Buy,
                Price::from("1.00"),
                Quantity::from("2.00"),
                1,
            ),
            0,
            0,
            1_000.into(),
            1_100.into(),
        );
        let deltas = OrderBookDeltas::new(subscription.instrument_id, vec![delta]);

        handle.record_book_deltas(&deltas, 1_200);

        assert!(
            handle.has_all_required_market_data(),
            "metadata-response book probes should pass after every installed source-owned target has book deltas"
        );
        assert_eq!(handle.book_evidence().deltas.len(), 1);
    }

    #[test]
    fn data_client_readiness_metadata_response_book_probe_passes_at_min_observed_targets() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
            book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(5),
            allow_metadata_target_sampling: Some(true),
            min_observed_targets: Some(2),
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness book handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-A.SOURCE"),
            InstrumentId::from("CONFIGURED-B.SOURCE"),
            InstrumentId::from("CONFIGURED-C.SOURCE"),
            InstrumentId::from("CONFIGURED-D.SOURCE"),
            InstrumentId::from("CONFIGURED-E.SOURCE"),
        ]);
        assert_eq!(installed.len(), 5);

        let record_delta = |subscription: &StrategyFreeReferenceQuoteSubscription| {
            let delta = OrderBookDelta::new(
                subscription.instrument_id,
                BookAction::Add,
                BookOrder::new(
                    OrderSide::Buy,
                    Price::from("1.00"),
                    Quantity::from("2.00"),
                    1,
                ),
                0,
                0,
                1_000.into(),
                1_100.into(),
            );
            let deltas = OrderBookDeltas::new(subscription.instrument_id, vec![delta]);
            handle.record_book_deltas(&deltas, 1_200);
        };

        assert!(
            !handle.has_all_required_market_data(),
            "book probe must not pass before any sampled target streams a delta"
        );

        record_delta(&installed[0]);
        assert!(
            !handle.has_all_required_market_data(),
            "book probe must keep waiting below min_observed_targets (1 of required 2)"
        );

        record_delta(&installed[1]);
        assert!(
            handle.has_all_required_market_data(),
            "book probe should pass once min_observed_targets sampled targets stream fresh deltas, without requiring every illiquid sampled instrument to tick"
        );
    }

    #[test]
    fn data_client_readiness_probe_rejects_zero_min_observed_targets() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
            book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(5),
            allow_metadata_target_sampling: Some(true),
            min_observed_targets: Some(0),
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        assert!(
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .is_err(),
            "min_observed_targets=0 must fail closed: a probe that observes nothing proves nothing"
        );
    }

    #[test]
    fn data_client_readiness_probe_fails_closed_when_min_observed_exceeds_sampled() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Book,
            book_type: Some(DataClientReadinessProbeBookType::L2Mbp),
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::MetadataResponse,
            max_metadata_quote_targets: Some(5),
            allow_metadata_target_sampling: Some(true),
            min_observed_targets: Some(4),
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: None,
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("metadata-response readiness book handle should build");
        let installed = handle.install_metadata_response_instrument_ids(vec![
            InstrumentId::from("CONFIGURED-A.SOURCE"),
            InstrumentId::from("CONFIGURED-B.SOURCE"),
        ]);

        assert!(
            installed.is_empty(),
            "install must fail closed when min_observed_targets exceeds the sampled target count"
        );
        assert!(
            !handle.has_all_required_market_data(),
            "probe must not pass after min_observed_targets exceeds the sampled targets"
        );
    }

    #[test]
    fn data_client_readiness_probe_times_out_without_market_data() {
        let mut loaded = fixture_loaded_config();
        let client = loaded
            .root
            .clients
            .get_mut("polymarket_main")
            .expect("fixture should include a data client");
        client.readiness_probe = Some(DataClientReadinessProbeBlock {
            market_data_kind: DataClientReadinessProbeMarketDataKind::Quote,
            book_type: None,
            quote_target_source: DataClientReadinessProbeQuoteTargetSource::Configured,
            max_metadata_quote_targets: None,
            allow_metadata_target_sampling: None,
            min_observed_targets: None,
            chunk_size: None,
            chunk_observation_window_seconds: None,
            quote_targets: Some(BTreeMap::from([(
                "configured_quote_probe".to_string(),
                DataClientReadinessProbeQuoteTargetBlock {
                    instrument_id: InstrumentId::from("REFERENCE.POLYMARKET"),
                },
            )])),
        });

        let handle =
            strategy_free_data_client_readiness_quote_probe_handle(&loaded, "polymarket_main")
                .expect("configured readiness quote handle should build");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("test runtime should build");

        let timed_out = runtime.block_on(async {
            tokio::time::timeout(
                Duration::from_millis(1),
                handle.wait_for_all_required_quotes(),
            )
            .await
            .is_err()
        });

        assert!(timed_out, "no-data probe wait must time out");
        assert_eq!(handle.observed_market_data_count(), 0);
        assert_eq!(handle.required_market_data_count(), 1);
        assert!(
            !handle.has_all_required_market_data(),
            "zero observed updates must not satisfy the data-client probe"
        );
    }

    #[test]
    fn runtime_redaction_value_buffers_zeroize_on_drop() {
        fn assert_zeroize_on_drop<T: zeroize::ZeroizeOnDrop>() {}
        fn redaction_values_field(runtime: &BoltV3LiveNodeRuntime) -> &Vec<Zeroizing<String>> {
            &runtime.redaction_values
        }

        assert_zeroize_on_drop::<Vec<Zeroizing<String>>>();
        let _ = redaction_values_field as fn(&BoltV3LiveNodeRuntime) -> &Vec<Zeroizing<String>>;
    }

    #[test]
    fn strategy_free_transport_config_preserves_identity_but_removes_strategy_instances() {
        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        assert!(
            !loaded.strategies.is_empty(),
            "fixture must include strategy config to prove strategy-free transport strips it"
        );

        let strategy_free_loaded = strategy_free_transport_loaded_config(&loaded);

        assert!(
            strategy_free_loaded.strategies.is_empty(),
            "strategy-free transport runtime must not register strategy actors"
        );
        assert_eq!(strategy_free_loaded.root_path, loaded.root_path);
        assert_eq!(
            strategy_free_loaded.config_bundle_checksum,
            loaded.config_bundle_checksum
        );
        assert_eq!(
            strategy_free_loaded.root.strategy_files,
            loaded.root.strategy_files
        );
        assert!(
            !loaded.strategies.is_empty(),
            "helper must not mutate the caller's loaded config"
        );
    }

    #[test]
    fn trade_transport_config_keeps_iv_only_source_clients() {
        let loaded = fixture_loaded_config_with_external_option_greeks_iv();

        let scoped =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect("IV source client must stay in scope");

        assert_eq!(scoped.root.clients.len(), 1);
        assert!(scoped.root.clients.contains_key("configured-client"));
        assert!(loaded.root.clients.contains_key("configured-client"));
    }

    #[test]
    fn trade_transport_config_prunes_unreferenced_hyperliquid_execution_clients_from_root() {
        let loaded =
            crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new("config/root.toml"))
                .expect("production root config should load");
        assert!(
            loaded.root.clients.contains_key("hyperliquid_execution"),
            "hyperliquid_execution should be configured in root.toml before transport pruning"
        );

        let scoped =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect("production trade transport scope should derive cleanly");

        assert!(
            !scoped.root.clients.contains_key("hyperliquid_execution"),
            "hyperliquid_execution is not strategy-referenced and must not reach live-node registration"
        );
    }

    #[test]
    fn trade_transport_config_rejects_multiple_referenced_execution_clients_for_same_venue() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        assert!(
            !loaded.strategies.is_empty(),
            "fixture config should include a strategy for venue-cardinality coverage"
        );
        let cloned_strategy = loaded.strategies[0].clone();
        loaded.strategies.truncate(1);
        loaded.strategies.push(cloned_strategy);
        loaded.strategies[0].config.execution_client_id = ClientId::from("hyperliquid_a");
        loaded.strategies[1].config.execution_client_id = ClientId::from("hyperliquid_b");
        loaded.root.clients.insert(
            "hyperliquid_a".to_string(),
            test_execution_client("HYPERLIQUID"),
        );
        loaded.root.clients.insert(
            "hyperliquid_b".to_string(),
            test_execution_client("HYPERLIQUID"),
        );

        let error =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect_err("multiple active execution clients for one venue must fail closed");
        let BoltV3LiveNodeError::LiveTransportScope { reason } = error else {
            panic!("expected LiveTransportScope error for duplicate execution venue");
        };
        assert!(
            reason.contains("multiple execution clients share venue `HYPERLIQUID`")
                && reason.contains("hyperliquid_a")
                && reason.contains("hyperliquid_b"),
            "duplicate execution venue error should identify venue and clients: {reason}"
        );
    }

    #[test]
    fn all_configured_mapping_rejects_multiple_execution_clients_for_same_venue_before_resolution()
    {
        let mut loaded = fixture_loaded_config();
        loaded.root.clients.clear();
        loaded.root.clients.insert(
            "hyperliquid_a".to_string(),
            test_execution_client("HYPERLIQUID"),
        );
        loaded.root.clients.insert(
            "hyperliquid_b".to_string(),
            test_execution_client("HYPERLIQUID"),
        );

        let error = match build_bolt_v3_all_configured_client_mapping_live_node_with_summary(
            &loaded,
            |_| false,
            |_, _| -> Result<String, String> {
                panic!("duplicate execution venue should fail before secret resolution")
            },
        ) {
            Ok(_) => panic!("all-configured mapping should reject duplicate execution venues"),
            Err(error) => error,
        };

        let BoltV3LiveNodeError::LiveTransportScope { reason } = error else {
            panic!("expected LiveTransportScope error for duplicate execution venue");
        };
        assert!(
            reason.contains("multiple execution clients share venue `HYPERLIQUID`")
                && reason.contains("hyperliquid_a")
                && reason.contains("hyperliquid_b"),
            "duplicate execution venue error should identify venue and clients: {reason}"
        );
    }

    #[test]
    fn trade_transport_config_keeps_strategy_and_root_substrate_clients() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let mut signal_client = loaded
            .root
            .clients
            .get("polymarket_main")
            .expect("fixture client should exist")
            .clone();
        signal_client.execution = None;
        signal_client.secrets = None;
        let unrelated_client = signal_client.clone();
        loaded
            .root
            .clients
            .insert("signal_data".to_string(), signal_client);
        loaded
            .root
            .clients
            .insert("unrelated_data".to_string(), unrelated_client);
        {
            let strategy = loaded
                .strategies
                .first_mut()
                .expect("fixture should include one strategy");
            strategy.config.signal_data.insert(
                "primary".to_string(),
                DataInstrumentBlock {
                    data_client_id: ClientId::from("signal_data"),
                    instrument_id: InstrumentId::from("SIGNAL.SOURCE"),
                },
            );
        }
        let mut rv_client = loaded
            .root
            .clients
            .get("polymarket_main")
            .expect("fixture client should exist")
            .clone();
        rv_client.execution = None;
        rv_client.secrets = None;
        loaded.root.clients.insert("rv_data".to_string(), rv_client);
        loaded
            .root
            .realized_volatility_surfaces
            .as_mut()
            .expect("fixture should include realized-volatility surfaces")
            .get_mut("configured_rv_surface")
            .expect("fixture should include configured RV surface")
            .sources
            .first_mut()
            .expect("fixture RV surface should include one source")
            .data_client_id = ClientId::from("rv_data");
        let mut gate_client = loaded
            .root
            .clients
            .get("polymarket_main")
            .expect("fixture client should exist")
            .clone();
        gate_client.execution = None;
        gate_client.secrets = None;
        loaded
            .root
            .clients
            .insert("gate_data".to_string(), gate_client);
        loaded
            .root
            .gate_providers
            .as_mut()
            .expect("fixture should include gate providers")
            .get_mut("resolution_oracle_primary")
            .expect("fixture should include target-referenced gate provider")
            .client_id = Some(ClientId::from("gate_data"));
        let mut outcome_group_client = loaded
            .root
            .clients
            .get("polymarket_main")
            .expect("fixture client should exist")
            .clone();
        outcome_group_client.execution = None;
        outcome_group_client.secrets = None;
        loaded
            .root
            .clients
            .insert("outcome_group_data".to_string(), outcome_group_client);
        loaded.root.outcome_group_sources = Some(vec![
            crate::bolt_v3_outcome_group_sources::OutcomeGroupSourceConfig {
                source_id: "configured_group_source".to_string(),
                client_id: ClientId::from("outcome_group_data"),
                kind: crate::bolt_v3_outcome_group_sources::OutcomeGroupSourceKind::GammaQuery,
                event_slugs: None,
                market_slugs: None,
                sports_market_types: None,
                gamma_query: Some(crate::bolt_v3_outcome_group_sources::GammaQueryBlock {
                    search: None,
                    event_query: None,
                    market_query: Some("configured outcome group".to_string()),
                    tag_id: None,
                    sports_market_types: None,
                    max_events: None,
                    max_markets: 1,
                }),
                question: None,
                expected_neg_risk_market_id: None,
                terminal_state_labels: None,
                max_markets: None,
                max_groups: None,
                enabled: true,
                freshness: None,
                order_constraints: None,
                role_bindings: None,
                settlement_rules: None,
            },
        ]);
        let mut outcome_group_strategy = loaded
            .strategies
            .first()
            .expect("fixture should include one strategy")
            .clone();
        outcome_group_strategy.config.strategy_instance_id = "configured_outcome_group".to_string();
        outcome_group_strategy.config.realized_volatility_surface_id = None;
        outcome_group_strategy.config.signal_data.clear();
        outcome_group_strategy.config.reference_current_price = None;
        outcome_group_strategy.config.resolution_data = None;
        outcome_group_strategy.config.target = toml::toml! {
            configured_target_id = "configured_outcome_group_target"
            kind = "static_outcome_group"
            rotating_market_family = "outcome_group"
            group_sources = ["configured_group_source"]
        }
        .into();
        loaded.strategies.push(outcome_group_strategy);

        let scoped =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect("strategy-bound transport scope should be derived from config");

        assert_eq!(scoped.root.clients.len(), 7);
        assert!(scoped.root.clients.contains_key("polymarket_main"));
        assert!(scoped.root.clients.contains_key("signal_data"));
        assert!(scoped.root.clients.contains_key("rv_data"));
        assert!(scoped.root.clients.contains_key("gate_data"));
        assert!(scoped.root.clients.contains_key("outcome_group_data"));
        assert!(scoped.root.clients.contains_key("chainlink_reference"));
        assert!(scoped.root.clients.contains_key("polyresearch_reference"));
        assert!(
            !scoped.root.clients.contains_key("unrelated_data"),
            "unrelated configured data clients must not block the selected trade path"
        );
        assert_eq!(scoped.strategies.len(), loaded.strategies.len());
        assert!(
            loaded.root.clients.contains_key("unrelated_data"),
            "helper must not mutate the caller's full client bundle"
        );
    }

    #[test]
    fn trade_transport_config_fails_closed_on_malformed_gate_subscription_target() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let strategy = loaded
            .strategies
            .first_mut()
            .expect("fixture should include one strategy");
        let resolution = strategy
            .config
            .target
            .as_table_mut()
            .and_then(|target| target.get_mut("gate_subscriptions"))
            .and_then(toml::Value::as_table_mut)
            .and_then(|subscriptions| subscriptions.get_mut("resolution"))
            .and_then(toml::Value::as_table_mut)
            .expect("fixture strategy should include resolution gate subscriptions");
        resolution.insert(
            "required".to_string(),
            toml::Value::String("not-a-bool".to_string()),
        );

        let error =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect_err("malformed gate subscription target must fail closed");
        let BoltV3LiveNodeError::LiveTransportScope { reason } = error else {
            panic!("expected LiveTransportScope error for malformed target");
        };
        assert!(
            reason.contains("gate_subscriptions") && reason.contains("not-a-bool"),
            "malformed target error should identify the gate subscription field: {reason}"
        );
    }

    #[test]
    fn trade_transport_config_fails_closed_on_unknown_gate_provider_reference() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let strategy = loaded
            .strategies
            .first_mut()
            .expect("fixture should include one strategy");
        let mapping = strategy
            .config
            .target
            .as_table_mut()
            .and_then(|target| target.get_mut("gate_subscriptions"))
            .and_then(toml::Value::as_table_mut)
            .and_then(|subscriptions| subscriptions.get_mut("resolution"))
            .and_then(toml::Value::as_table_mut)
            .and_then(|resolution| resolution.get_mut("market_mappings"))
            .and_then(toml::Value::as_array_mut)
            .and_then(|mappings| mappings.first_mut())
            .and_then(toml::Value::as_table_mut)
            .expect("fixture strategy should include a resolution gate mapping");
        mapping.insert(
            "provider_id".to_string(),
            toml::Value::String("missing_gate_provider".to_string()),
        );

        let error =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect_err("unknown gate provider reference must fail closed");
        let BoltV3LiveNodeError::LiveTransportScope { reason } = error else {
            panic!("expected LiveTransportScope error for missing gate provider");
        };
        assert!(
            reason.contains("missing_gate_provider")
                && reason.contains("[gate_providers.missing_gate_provider]"),
            "missing provider error should identify the unresolved provider: {reason}"
        );
    }

    #[test]
    fn trade_transport_config_allows_no_provider_gate_subscription_without_gate_providers() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded.root.gate_providers = None;
        let strategy = loaded
            .strategies
            .first_mut()
            .expect("fixture should include one strategy");
        let subscriptions = strategy
            .config
            .target
            .as_table_mut()
            .and_then(|target| target.get_mut("gate_subscriptions"))
            .and_then(toml::Value::as_table_mut)
            .expect("fixture strategy should include gate subscriptions");
        subscriptions.clear();
        subscriptions.insert(
            "resolution".to_string(),
            toml::toml! {
                required = false
                allow_no_resolution = true
            }
            .into(),
        );

        let scoped =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect("provider-free gate subscription should not require [gate_providers]");
        assert!(
            !scoped.root.clients.contains_key("gate_data"),
            "provider-free gate subscription must not retain a gate provider data client"
        );
    }

    #[test]
    fn trade_transport_subscribed_retains_enabled_rv_sources_from_unreferenced_surfaces() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded
            .root
            .clients
            .insert("rv_only_data".to_string(), test_data_client("OKX"));
        insert_test_rv_surface(
            &mut loaded,
            "orphan_rv_surface",
            vec![test_rv_source(
                "orphan_midpoint",
                "rv_only_data",
                "CONFIGURED_ASSET-USDT-PERP.OKX",
                true,
            )],
        );

        let scoped =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect("RV-only source client must stay in trade transport scope");

        assert!(
            scoped.root.clients.contains_key("rv_only_data"),
            "trade transport must retain enabled RV sources even when no strategy names the surface"
        );
    }

    #[test]
    fn trade_transport_subscribed_retains_union_of_enabled_rv_sources_across_surfaces() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded
            .root
            .clients
            .insert("rv_union_a".to_string(), test_data_client("OKX"));
        loaded
            .root
            .clients
            .insert("rv_union_b".to_string(), test_data_client("OKX"));
        insert_test_rv_surface(
            &mut loaded,
            "union_rv_surface_a",
            vec![test_rv_source(
                "union_midpoint_a",
                "rv_union_a",
                "CONFIGURED_ASSET-USDT-UNION-A.OKX",
                true,
            )],
        );
        insert_test_rv_surface(
            &mut loaded,
            "union_rv_surface_b",
            vec![test_rv_source(
                "union_midpoint_b",
                "rv_union_b",
                "CONFIGURED_ASSET-USDT-UNION-B.OKX",
                true,
            )],
        );

        let scoped =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect("trade transport should retain every enabled RV source client");

        assert!(scoped.root.clients.contains_key("rv_union_a"));
        assert!(scoped.root.clients.contains_key("rv_union_b"));
    }

    #[test]
    fn trade_transport_subscribed_dedupes_duplicate_rv_source_clients() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded
            .root
            .clients
            .insert("shared_rv_data".to_string(), test_data_client("OKX"));
        insert_test_rv_surface(
            &mut loaded,
            "shared_rv_surface_a",
            vec![test_rv_source(
                "shared_midpoint_a",
                "shared_rv_data",
                "CONFIGURED_ASSET-USDT-SHARED-A.OKX",
                true,
            )],
        );
        insert_test_rv_surface(
            &mut loaded,
            "shared_rv_surface_b",
            vec![test_rv_source(
                "shared_midpoint_b",
                "shared_rv_data",
                "CONFIGURED_ASSET-USDT-SHARED-B.OKX",
                true,
            )],
        );

        let keys =
            trade_transport_client_keys(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect("duplicate RV source clients should still produce transport keys");

        assert_eq!(
            keys.iter()
                .filter(|client_key| client_key.as_str() == "shared_rv_data")
                .count(),
            1,
            "RV source client retention must be a set across surfaces"
        );
    }

    #[test]
    fn trade_transport_subscribed_retains_enabled_unsupported_kind_rv_sources() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded
            .root
            .clients
            .insert("mark_rv_data".to_string(), test_data_client("OKX"));
        let mut mark_source = test_rv_source(
            "mark_midpoint",
            "mark_rv_data",
            "CONFIGURED_ASSET-USDT-MARK.OKX",
            true,
        );
        mark_source.source_class = RealizedVolatilitySourceClassBlock::Mark;
        mark_source.sample_kind = RealizedVolatilitySampleKindBlock::Mark;
        insert_test_rv_surface(&mut loaded, "mark_rv_surface", vec![mark_source]);

        let scoped = trade_transport_loaded_config(
            &loaded,
            RealizedVolatilityTransportScope::Subscribed,
        )
        .expect(
            "transport retention must over-retain enabled RV sources before runtime validation",
        );

        assert!(
            scoped.root.clients.contains_key("mark_rv_data"),
            "transport retention must include every enabled RV source client, even if later validation rejects the source kind"
        );
    }

    #[test]
    fn trade_transport_subscribed_excludes_disabled_rv_sources() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        loaded
            .root
            .clients
            .insert("disabled_rv_data".to_string(), test_data_client("OKX"));
        insert_test_rv_surface(
            &mut loaded,
            "disabled_rv_surface",
            vec![test_rv_source(
                "disabled_midpoint",
                "disabled_rv_data",
                "CONFIGURED_ASSET-USDT-DISABLED.OKX",
                false,
            )],
        );

        let scoped =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect("disabled RV sources must not affect transport derivation");

        assert!(
            !scoped.root.clients.contains_key("disabled_rv_data"),
            "disabled RV source clients must not be retained"
        );
    }

    #[test]
    fn trade_transport_subscribed_zero_strategies_skips_broken_rv_clients() {
        let mut loaded = fixture_loaded_config();
        loaded.root.clients.remove("okx_data");

        let scoped =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect(
                    "zero-strategy Subscribed transport must not validate or retain RV sources",
                );

        assert!(
            !scoped.root.clients.contains_key("okx_data"),
            "zero-strategy transport must not pull in RV source clients"
        );
    }

    #[test]
    fn trade_transport_not_subscribed_ignores_broken_rv_only_clients() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        insert_test_rv_surface(
            &mut loaded,
            "broken_rv_surface",
            vec![test_rv_source(
                "broken_midpoint",
                "missing_rv_data",
                "CONFIGURED_ASSET-USDT-MISSING.OKX",
                true,
            )],
        );

        let scoped =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::NotSubscribed)
                .expect("strategy-free transport must not validate or retain RV-only sources");

        assert!(
            !scoped.root.clients.contains_key("missing_rv_data"),
            "NotSubscribed transport must not pull in RV source clients"
        );
    }

    #[test]
    fn trade_transport_handles_absent_and_empty_rv_surface_maps() {
        let mut loaded = fixture_loaded_config();
        loaded.root.realized_volatility_surfaces = None;
        let none_keys =
            trade_transport_client_keys(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect("absent RV surfaces should still derive transport keys");
        assert!(none_keys.is_empty());

        loaded.root.realized_volatility_surfaces = Some(BTreeMap::new());
        let empty_keys =
            trade_transport_client_keys(&loaded, RealizedVolatilityTransportScope::Subscribed)
                .expect("empty RV surfaces should still derive transport keys");
        assert!(empty_keys.is_empty());
    }

    #[test]
    fn registration_rejects_rv_source_missing_from_node_transport() {
        let loaded = loaded_config_with_rv_only_source();
        let pruned_loaded =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::NotSubscribed)
                .expect("strategy-free transport should ignore RV-only sources");
        assert!(
            !pruned_loaded.root.clients.contains_key("rv_only_data"),
            "test setup must build a node transport that pruned the RV-only source"
        );
        let mut node = make_bolt_v3_live_node_builder(&pruned_loaded)
            .expect("test LiveNodeBuilder should construct")
            .build()
            .expect("test LiveNode should build");
        let writer: Arc<dyn BoltV3DecisionEvidenceWriter> =
            Arc::new(NoStrategyDecisionEvidenceWriter);

        let error = register_bolt_v3_strategies_on_node_with_bindings(
            &mut node,
            &loaded,
            &ResolvedBoltV3Secrets {
                clients: Default::default(),
            },
            &[],
            test_registration_controls(writer.clone()),
            writer,
        )
        .expect_err("registration must fail before RV runtime subscribes a pruned client");

        assert!(matches!(
            error,
            BoltV3StrategyRegistrationError::RealizedVolatilityRuntime { message }
                if message.contains("not registered on this node's transport")
                    && message.contains("rv_only_data")
        ));
    }

    #[test]
    fn registration_with_iv_runtime_rejects_rv_source_missing_from_node_transport() {
        let loaded = loaded_config_with_rv_only_source();
        let pruned_loaded =
            trade_transport_loaded_config(&loaded, RealizedVolatilityTransportScope::NotSubscribed)
                .expect("strategy-free transport should ignore RV-only sources");
        let mut node = make_bolt_v3_live_node_builder(&pruned_loaded)
            .expect("test LiveNodeBuilder should construct")
            .build()
            .expect("test LiveNode should build");
        let writer: Arc<dyn BoltV3DecisionEvidenceWriter> =
            Arc::new(NoStrategyDecisionEvidenceWriter);
        let iv_runtime = IvRuntimeEngine::from_iv_root(&IvRootConfig {
            schema_version: 1,
            profiles: Vec::new(),
        })
        .expect("empty IV runtime should construct");

        let error = register_bolt_v3_strategies_on_node_with_iv_runtime_bindings(
            &mut node,
            &loaded,
            &ResolvedBoltV3Secrets {
                clients: Default::default(),
            },
            &[],
            test_registration_controls(writer.clone()),
            writer,
            &iv_runtime,
        )
        .expect_err("IV-runtime registration must fail before RV subscribes a pruned client");

        assert!(matches!(
            error,
            BoltV3StrategyRegistrationError::RealizedVolatilityRuntime { message }
                if message.contains("not registered on this node's transport")
                    && message.contains("rv_only_data")
        ));
    }

    #[test]
    fn data_client_probe_config_keeps_only_selected_data_client() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let mut secondary = loaded
            .root
            .clients
            .get("polymarket_main")
            .expect("fixture client should exist")
            .clone();
        secondary.execution = None;
        secondary.secrets = None;
        loaded
            .root
            .clients
            .insert("secondary_data".to_string(), secondary);

        let probe_loaded = data_client_probe_loaded_config(&loaded, "secondary_data")
            .expect("selected data client should produce a scoped probe config");

        assert!(
            probe_loaded.strategies.is_empty(),
            "adapter mapping must drop strategy targets that do not reference the selected probe client"
        );
        assert_eq!(probe_loaded.root_path, loaded.root_path);
        assert_eq!(
            probe_loaded.config_bundle_checksum,
            loaded.config_bundle_checksum
        );
        assert_eq!(probe_loaded.root.clients.len(), 1);
        assert!(probe_loaded.root.clients.contains_key("secondary_data"));
        assert!(
            loaded.root.clients.contains_key("polymarket_main"),
            "helper must not mutate the caller's full client bundle"
        );
    }

    #[test]
    fn data_client_census_report_sorts_and_dedupes_instrument_ids() {
        let unsorted = data_client_census_report(
            "bybit_data",
            vec![
                "ETH/USDT.BYBIT".to_string(),
                "BTC/USDT.BYBIT".to_string(),
                "ETH/USDT.BYBIT".to_string(),
            ],
        )
        .expect("non-empty census should build");
        let sorted = data_client_census_report(
            "bybit_data",
            vec!["BTC/USDT.BYBIT".to_string(), "ETH/USDT.BYBIT".to_string()],
        )
        .expect("deduped census should build");

        assert_eq!(unsorted.cached_instrument_count, 2);
        assert_eq!(
            unsorted.cached_instrument_ids_sha256,
            sorted.cached_instrument_ids_sha256
        );
    }

    #[test]
    fn data_client_census_report_rejects_empty_cache() {
        let error = data_client_census_report("bybit_data", Vec::new())
            .expect_err("empty instrument cache must fail closed");

        assert!(matches!(
            error,
            BoltV3LiveNodeError::StrategyFreeDataClientProbeFailed { reason }
                if reason.contains("zero cached instruments")
        ));
    }

    #[test]
    fn data_client_probe_adapter_mapping_drops_unrelated_strategy_targets() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let mut secondary = loaded
            .root
            .clients
            .get("polymarket_main")
            .expect("fixture client should exist")
            .clone();
        secondary.execution = None;
        secondary.secrets = None;
        loaded
            .root
            .clients
            .insert("secondary_data".to_string(), secondary);

        let probe_loaded = data_client_probe_loaded_config(&loaded, "secondary_data")
            .expect("selected data client should produce a scoped probe config");

        assert!(
            probe_loaded.strategies.is_empty(),
            "probe mapping input must drop strategy targets that reference clients outside the scoped probe"
        );
        strategy_free_transport_adapter_configs(
            &probe_loaded,
            &crate::bolt_v3_secrets::ResolvedBoltV3Secrets {
                clients: Default::default(),
            },
        )
        .expect("scoped data-client adapter mapping must not fail on unrelated strategies");
    }

    #[test]
    fn data_client_probe_runtime_clears_strategies_after_adapter_mapping() {
        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");

        let probe_loaded = data_client_probe_loaded_config(&loaded, "polymarket_main")
            .expect("selected data client should produce a scoped probe config");
        let runtime_loaded = strategy_free_transport_loaded_config(&probe_loaded);

        assert!(
            !probe_loaded.strategies.is_empty(),
            "probe adapter mapping input must keep strategies for provider-owned data filters"
        );
        assert!(
            runtime_loaded.strategies.is_empty(),
            "strategy-free data-client probes must not register strategy actors"
        );
        assert_eq!(runtime_loaded.root.clients.len(), 1);
        assert!(runtime_loaded.root.clients.contains_key("polymarket_main"));
        assert!(
            !runtime_loaded.root.clients.contains_key("okx_data"),
            "strategy-free data-client probes must not pull in configured RV sources"
        );
    }

    #[test]
    fn strategy_free_adapter_mapping_preserves_strategy_derived_market_filters() {
        use crate::{
            bolt_v3_providers::{
                binance::ResolvedBoltV3BinanceSecrets, chainlink::ResolvedBoltV3ChainlinkSecrets,
                chainlink_reference::ResolvedBoltV3ChainlinkReferenceSecrets,
                polymarket::ResolvedBoltV3PolymarketSecrets,
                polyresearch::ResolvedBoltV3PolyResearchSecrets,
            },
            bolt_v3_secrets::{ResolvedBoltV3ClientSecrets, ResolvedBoltV3Secrets},
        };
        use nautilus_polymarket::config::PolymarketDataClientConfig;
        use std::{collections::BTreeMap, sync::Arc};

        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let mut clients: BTreeMap<String, ResolvedBoltV3ClientSecrets> = BTreeMap::new();
        clients.insert(
            "polymarket_main".to_string(),
            Arc::new(ResolvedBoltV3PolymarketSecrets {
                private_key: zeroize::Zeroizing::new("fixture-poly-private-key".to_string()),
                api_key: zeroize::Zeroizing::new("fixture-poly-api-key".to_string()),
                api_secret: zeroize::Zeroizing::new("fixture-poly-api-secret".to_string()),
                passphrase: zeroize::Zeroizing::new("fixture-poly-passphrase".to_string()),
            }),
        );
        clients.insert(
            "binance_reference".to_string(),
            Arc::new(ResolvedBoltV3BinanceSecrets {
                api_key: zeroize::Zeroizing::new("fixture-binance-api-key".to_string()),
                api_secret: zeroize::Zeroizing::new("fixture-binance-api-secret".to_string()),
            }),
        );
        clients.insert(
            "chainlink_strike".to_string(),
            Arc::new(ResolvedBoltV3ChainlinkSecrets {
                api_key: zeroize::Zeroizing::new("fixture-chainlink-api-key".to_string()),
                api_secret: zeroize::Zeroizing::new("fixture-chainlink-api-secret".to_string()),
            }),
        );
        clients.insert(
            "chainlink_reference".to_string(),
            Arc::new(ResolvedBoltV3ChainlinkReferenceSecrets {
                api_key: zeroize::Zeroizing::new("fixture-chainlink-reference-api-key".to_string()),
                api_secret: zeroize::Zeroizing::new(
                    "fixture-chainlink-reference-api-secret".to_string(),
                ),
            }),
        );
        clients.insert(
            "polyresearch_reference".to_string(),
            Arc::new(ResolvedBoltV3PolyResearchSecrets {
                api_key: zeroize::Zeroizing::new("fixture-polyresearch-api-key".to_string()),
            }),
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let adapters = strategy_free_transport_adapter_configs(&loaded, &resolved)
            .expect("strategy-free adapter mapping should retain market identity filters");
        let polymarket = adapters
            .clients
            .get("polymarket_main")
            .expect("polymarket_main must be mapped");
        let data = polymarket
            .data
            .as_ref()
            .expect("polymarket data config must be mapped")
            .config_as::<PolymarketDataClientConfig>()
            .expect("polymarket data config should downcast");

        assert_eq!(
            data.filters.len(),
            1,
            "strategy-free adapter mapping must keep strategy-derived provider filters"
        );
        assert_eq!(
            data.filters[0]
                .market_slugs()
                .expect("strategy-free data config must keep configured target slug filters")
                .len(),
            2
        );
    }

    #[test]
    fn live_node_config_maps_trader_id_and_environment_from_v3_root() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);

        assert_eq!(cfg.trader_id, TraderId::from("BOLT-001"));
        assert_eq!(cfg.environment, Environment::Live);
        assert_eq!(cfg.timeout_connection, Duration::from_secs(30));
        assert_eq!(cfg.timeout_reconciliation, Duration::from_secs(60));
        assert_eq!(cfg.timeout_portfolio, Duration::from_secs(10));
        assert_eq!(cfg.timeout_disconnection, Duration::from_secs(10));
        assert_eq!(cfg.delay_post_stop, Duration::from_secs(5));
        assert_eq!(cfg.timeout_shutdown, Duration::from_secs(10));
    }

    #[test]
    fn live_node_builder_rejects_backtest_environment_before_registration() {
        let loaded = fixture_loaded_config();
        let make_error = || {
            let mut cfg = make_live_node_config(&loaded);
            cfg.environment = Environment::Backtest;
            make_bolt_v3_live_node_builder_from_config(cfg)
                .expect_err("NT LiveNodeBuilder must reject Backtest environment")
        };

        let rendered = BoltV3LiveNodeError::BuilderConstruction(make_error()).to_string();
        assert_eq!(
            rendered
                .matches("LiveNodeBuilder construction failed")
                .count(),
            1,
            "builder-construction Display should not duplicate layer prefixes: {rendered}"
        );
        assert!(
            rendered.contains("Backtest environment"),
            "builder-construction failure should identify the invalid environment: {rendered}"
        );

        let BoltV3LiveNodeBuilderError::BuilderConstruction { source } = make_error();
        assert!(
            source.to_string().contains("Backtest environment"),
            "builder-construction failure should identify the invalid environment: {source}"
        );
    }

    #[test]
    fn combined_run_and_runtime_capture_shutdown_failure_preserves_both_error_types() {
        let error = classify_live_node_run_and_capture_shutdown(
            Err(anyhow::anyhow!("runner failed")),
            Err(anyhow::anyhow!("capture shutdown failed")),
        )
        .expect_err("combined failure must surface a bolt-v3 live-node error");

        let source = std::error::Error::source(&error)
            .expect("compound failure should expose the runner error as its source");
        assert_eq!(source.to_string(), "runner failed");

        match error {
            BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown {
                run_error,
                shutdown_error,
            } => {
                assert_eq!(run_error.to_string(), "runner failed");
                assert_eq!(shutdown_error.to_string(), "capture shutdown failed");
            }
            other => panic!(
                "combined runner/capture-shutdown failure must preserve both \
                 error categories, got {other:?}"
            ),
        }
    }

    #[test]
    fn live_node_config_top_level_residuals_are_disabled_or_empty() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);

        assert!(cfg.instance_id.is_none());
        assert!(cfg.cache.is_none());
        assert!(cfg.msgbus.is_none());
        assert!(cfg.portfolio.is_none());
        assert!(cfg.emulator.is_none());
        assert!(cfg.streaming.is_none());
        assert!(!cfg.loop_debug);
        assert!(cfg.data_clients.is_empty());
        assert!(cfg.exec_clients.is_empty());
    }

    #[test]
    fn live_node_config_maps_zero_lookback_to_unbounded_reconciliation() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);
        assert_eq!(cfg.exec_engine.reconciliation_lookback_mins, None);
    }

    #[test]
    fn strategy_free_timeout_sums_fail_closed_on_overflow() {
        let mut loaded = fixture_loaded_config();
        loaded.root.nautilus.timeout_connection_secs = u64::MAX;
        loaded.root.nautilus.timeout_reconciliation_secs = 1;
        let start_error = strategy_free_start_timeout_secs(&loaded)
            .expect_err("strategy-free start timeout overflow must fail closed");
        assert!(
            matches!(
                start_error,
                BoltV3LiveNodeError::StrategyFreeStartTimeoutOverflow
            ),
            "expected start timeout overflow rejection, got {start_error:?}"
        );

        loaded.root.nautilus.timeout_disconnection_secs = u64::MAX;
        loaded.root.nautilus.delay_post_stop_secs = 1;
        let stop_error = strategy_free_stop_timeout_secs(&loaded)
            .expect_err("strategy-free stop timeout overflow must fail closed");
        assert!(
            matches!(
                stop_error,
                BoltV3LiveNodeError::StrategyFreeStopTimeoutOverflow
            ),
            "expected stop timeout overflow rejection, got {stop_error:?}"
        );
    }

    #[test]
    fn live_node_config_maps_explicit_nt_runtime_defaults_from_v3_root() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);

        assert!(cfg.data_engine.time_bars_build_with_no_updates);
        assert!(cfg.data_engine.time_bars_timestamp_on_close);
        assert!(!cfg.data_engine.time_bars_skip_first_non_full_bar);
        assert_eq!(
            cfg.data_engine.time_bars_interval_type,
            nautilus_model::enums::BarIntervalType::LeftOpen
        );
        assert_eq!(cfg.data_engine.time_bars_build_delay, 0);
        assert!(cfg.data_engine.time_bars_origin_offset.is_empty());
        assert!(!cfg.data_engine.validate_data_sequence);
        assert!(!cfg.data_engine.buffer_deltas);
        assert!(!cfg.data_engine.emit_quotes_from_book);
        assert!(!cfg.data_engine.emit_quotes_from_book_depths);
        assert_eq!(cfg.data_engine.external_clients, None);
        assert!(!cfg.data_engine.debug);
        assert!(!cfg.data_engine.graceful_shutdown_on_error);
        assert_eq!(cfg.data_engine.qsize, 100_000);
        assert!(cfg.exec_engine.load_cache);
        assert!(!cfg.exec_engine.snapshot_orders);
        assert!(!cfg.exec_engine.snapshot_positions);
        assert_eq!(cfg.exec_engine.snapshot_positions_interval_secs, None);
        assert_eq!(cfg.exec_engine.external_clients, None);
        assert!(!cfg.exec_engine.debug);
        assert!(cfg.exec_engine.reconciliation);
        assert_eq!(cfg.exec_engine.reconciliation_startup_delay_secs, 10.0);
        assert_eq!(cfg.exec_engine.reconciliation_lookback_mins, None);
        assert_eq!(cfg.exec_engine.reconciliation_instrument_ids, None);
        assert!(!cfg.exec_engine.filter_unclaimed_external_orders);
        assert!(!cfg.exec_engine.filter_position_reports);
        assert_eq!(cfg.exec_engine.filtered_client_order_ids, None);
        assert!(cfg.exec_engine.generate_missing_orders);
        assert_eq!(cfg.exec_engine.inflight_check_interval_ms, 2_000);
        assert_eq!(cfg.exec_engine.inflight_check_threshold_ms, 5_000);
        assert_eq!(cfg.exec_engine.inflight_check_retries, 5);
        assert_eq!(cfg.exec_engine.open_check_interval_secs, None);
        assert_eq!(cfg.exec_engine.open_check_lookback_mins, Some(60));
        assert_eq!(cfg.exec_engine.open_check_threshold_ms, 5_000);
        assert_eq!(cfg.exec_engine.open_check_missing_retries, 5);
        assert!(cfg.exec_engine.open_check_open_only);
        assert_eq!(cfg.exec_engine.max_single_order_queries_per_cycle, 10);
        assert_eq!(cfg.exec_engine.single_order_query_delay_ms, 100);
        assert_eq!(cfg.exec_engine.position_check_interval_secs, None);
        assert_eq!(cfg.exec_engine.position_check_lookback_mins, 60);
        assert_eq!(cfg.exec_engine.position_check_threshold_ms, 5_000);
        assert_eq!(cfg.exec_engine.position_check_retries, 3);
        assert_eq!(cfg.exec_engine.purge_closed_orders_interval_mins, None);
        assert_eq!(cfg.exec_engine.purge_closed_orders_buffer_mins, None);
        assert_eq!(cfg.exec_engine.purge_closed_positions_interval_mins, None);
        assert_eq!(cfg.exec_engine.purge_closed_positions_buffer_mins, None);
        assert_eq!(cfg.exec_engine.purge_account_events_interval_mins, None);
        assert_eq!(cfg.exec_engine.purge_account_events_lookback_mins, None);
        assert!(!cfg.exec_engine.purge_from_database);
        assert_eq!(cfg.exec_engine.own_books_audit_interval_secs, None);
        assert!(!cfg.exec_engine.graceful_shutdown_on_error);
        assert_eq!(cfg.exec_engine.qsize, 100_000);
        assert!(!cfg.exec_engine.allow_overfills);
        assert!(!cfg.exec_engine.manage_own_order_books);
        assert!(!cfg.risk_engine.bypass);
        assert_eq!(cfg.risk_engine.max_order_submit_rate, "40/00:01:00");
        assert_eq!(cfg.risk_engine.max_order_modify_rate, "40/00:01:00");
        assert!(cfg.risk_engine.max_notional_per_order.is_empty());
        assert!(!cfg.risk_engine.debug);
        assert!(!cfg.risk_engine.graceful_shutdown_on_error);
        assert_eq!(cfg.risk_engine.qsize, 100_000);
    }

    #[test]
    fn live_node_config_maps_explicit_nt_risk_debug_from_v3_root() {
        let mut loaded = fixture_loaded_config();
        loaded.root.risk.nautilus.debug = true;

        let cfg = make_live_node_config(&loaded);

        assert!(cfg.risk_engine.debug);
    }

    #[test]
    fn live_node_config_maps_explicit_nt_data_engine_debug_from_v3_root() {
        let mut loaded = fixture_loaded_config();
        loaded.root.nautilus.data_engine.debug = true;

        let cfg = make_live_node_config(&loaded);

        assert!(cfg.data_engine.debug);
    }

    #[test]
    fn live_node_config_maps_non_empty_nt_max_notional_per_order() {
        let mut loaded = fixture_loaded_config();
        loaded
            .root
            .risk
            .nautilus
            .max_notional_per_order
            .insert("REFERENCE.SOURCE".to_string(), "12345.00".to_string());
        loaded
            .root
            .risk
            .nautilus
            .max_notional_per_order
            .insert("SECONDARY.SOURCE".to_string(), "25000.50".to_string());
        let cfg = make_live_node_config(&loaded);

        assert_eq!(
            cfg.risk_engine
                .max_notional_per_order
                .get("REFERENCE.SOURCE"),
            Some(&"12345.00".to_string())
        );
        assert_eq!(
            cfg.risk_engine
                .max_notional_per_order
                .get("SECONDARY.SOURCE"),
            Some(&"25000.50".to_string())
        );
    }

    #[test]
    fn venue_spendability_source_config_reads_configured_capital_pool_source() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let mut loaded = fixture_loaded_config();
        write_venue_spendability_source(&mut loaded, temp.path(), 1_500, "20", "12");
        let config = position_sizer_venue_spendability_source_config_from_loaded(&loaded)
            .expect("source config should build")
            .expect("fixture should configure source");

        let snapshot = position_sizer_venue_spendability_snapshot_from_source_config(&config)
            .expect("configured source should be accepted");

        assert_eq!(snapshot.source, "operator_venue_spendability");
        assert_eq!(snapshot.spendable_collateral, Decimal::from(20));
        assert_eq!(snapshot.collateral_allowance, Decimal::from(12));
    }

    #[test]
    fn venue_spendability_source_config_fails_closed_on_sha_mismatch() {
        let temp = tempfile::tempdir().expect("tempdir should create");
        let mut loaded = fixture_loaded_config();
        write_venue_spendability_source(&mut loaded, temp.path(), 1_500, "20", "12");
        let mut config = position_sizer_venue_spendability_source_config_from_loaded(&loaded)
            .expect("source config should build")
            .expect("fixture should configure source");
        config.expected_sha256 =
            "0000000000000000000000000000000000000000000000000000000000000000".to_string();

        let error = position_sizer_venue_spendability_snapshot_from_source_config(&config)
            .expect_err("hash mismatch must fail closed");
        let rendered = error.to_string();

        assert!(
            rendered.contains("position sizer venue spendability source rejected")
                && rendered.contains("Sha256Mismatch"),
            "startup error should name rejected spendability evidence, got: {rendered}"
        );
    }

    #[test]
    fn live_node_config_maps_log_levels_from_uppercase_strings() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);
        assert_eq!(cfg.logging.stdout_level, log::LevelFilter::Info);
        assert_eq!(cfg.logging.fileout_level, log::LevelFilter::Info);
    }

    #[test]
    fn live_node_config_logger_literal_does_not_inherit_nt_defaults() {
        let src = include_str!("bolt_v3_live_node.rs");
        let logging_literal = src
            .split("let logging = LoggerConfig {")
            .nth(1)
            .expect("logger config literal must exist")
            .split("let nautilus =")
            .next()
            .expect("logger config literal must precede nautilus config");

        // Field-add drift is caught by Rust struct literal exhaustiveness; this
        // guards against silently re-introducing inherited NT defaults.
        assert!(
            !logging_literal.contains(concat!("..", "Default::default()")),
            "LoggerConfig must set every pinned NT field explicitly"
        );
    }

    #[test]
    fn live_node_config_maps_explicit_logger_residuals_in_builder_path() {
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);

        assert!(cfg.logging.component_level.is_empty());
        assert!(!cfg.logging.log_components_only);
        assert!(cfg.logging.is_colored);
        assert!(!cfg.logging.print_config);
        assert!(!cfg.logging.use_tracing);
        assert!(!cfg.logging.bypass_logging);
        assert!(cfg.logging.file_config.is_none());
        assert!(!cfg.logging.clear_log_file);
    }

    #[test]
    fn live_node_config_suppresses_nt_credential_module_logs_to_warn() {
        // Regression for the slice-7 review finding: NT's
        // `nautilus_polymarket::common::credential` and
        // `nautilus_binance::common::credential` modules log credential
        // material at info-level. Bolt-v3 forces those targets to
        // `Warn` even when the root TOML log level is `Info`, so the
        // logger filter must contain both module paths with at most
        // `Warn` regardless of the configured root level.
        let loaded = fixture_loaded_config();
        let cfg = make_live_node_config(&loaded);

        for module_path in crate::bolt_v3_providers::credential_log_modules() {
            let key = Ustr::from(module_path);
            let level = cfg
                .logging
                .module_level
                .get(&key)
                .copied()
                .unwrap_or_else(|| panic!("logger module_level missing `{module_path}`"));
            assert!(
                level <= log::LevelFilter::Warn,
                "credential module `{module_path}` filter must be Warn or stricter, got {level:?}"
            );
        }
    }

    #[test]
    fn secret_resolver_setup_variant_renders_clean_message_without_empty_client_path() {
        // Per #255-2: before this fix, session-construction failure was
        // mapped into `BoltV3SecretError` with empty `client_key` and
        // `ssm_path`, rendering as a confusing
        // an empty client key in the secret-path template. The dedicated
        // `BoltV3LiveNodeError::SecretResolverSetup(SecretError)` variant
        // gives operators a clean, accurate message that does not
        // pretend a client or SSM path is involved (none is — the
        // failure happens before any path is read).
        let inner = crate::secrets::SecretError::for_test(
            "failed to build Tokio runtime for SSM resolver session: simulated".to_string(),
        );
        let err = BoltV3LiveNodeError::SecretResolverSetup(inner);
        let rendered = format!("{err}");
        assert!(
            !rendered.contains(".secrets.ssm_resolver_session"),
            "SecretResolverSetup must not render through the client/SSM-path template"
        );
        assert!(
            !rendered.contains("ssm_path"),
            "SecretResolverSetup must not include an empty ssm_path field"
        );
        assert!(
            rendered.contains("SSM resolver session"),
            "SecretResolverSetup message must name the resolver-session setup boundary"
        );
        assert!(
            rendered.contains("simulated"),
            "SecretResolverSetup must surface the wrapped SecretError"
        );
        let source = std::error::Error::source(&err);
        assert!(
            source.is_some(),
            "SecretResolverSetup must report its wrapped SecretError via \
             std::error::Error::source"
        );
    }
}
