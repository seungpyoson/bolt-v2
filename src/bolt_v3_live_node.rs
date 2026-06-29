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
#[cfg(test)]
use risk_admission_loss::capital_admission_venue_spendability_snapshot_from_source_config;
use risk_admission_loss::{
    BoltV3CapitalAdmissionVenueSpendabilitySourceConfig, BoltV3SubmitReservationRecoveryConfig,
    capital_admission_config_from_loaded, capital_admission_runtime_feed_config_from_loaded,
    capital_admission_venue_spendability_source_config_from_loaded,
    configure_bolt_v3_kill_switch_loss_protection, loss_governor_halt_action_handler_from_node,
    loss_governor_halt_action_policy_from_loaded, loss_governor_policy_from_loaded,
    loss_governor_runtime_feed_config_from_loaded, order_reject_observer_account_id_from_loaded,
    recover_kill_switch_state_before_live_node_build,
    refresh_capital_admission_venue_spendability_from_source,
    submit_reservation_recovery_config_from_loaded, sync_nt_trading_state_for_kill_switch,
    wire_bolt_v3_loss_protection_runtime,
};
pub use secrets_builders::{
    build_bolt_v3_live_node_with_resolved, build_bolt_v3_strategy_free_data_client_probe_live_node,
    build_bolt_v3_strategy_free_live_node, build_bolt_v3_strategy_free_live_node_with_resolved,
    build_bolt_v3_strategy_free_live_node_with_summary,
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

#[cfg(test)]
mod tests;
