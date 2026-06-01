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
//! external network connection. Opt-in controlled-connect/no-submit
//! readiness boundaries may open adapter sockets. The production
//! trading runner entrypoint is [`run_bolt_v3_live_node`], which first
//! applies the bolt-v3 live canary gate. The no-submit readiness path
//! builds a strategy-free node before using NT's supported runner loop
//! with handle-driven stop; its dedicated reference quote probe calls
//! only NT quote subscribe/unsubscribe APIs for configured
//! `[reference_data]` instruments. This module still never constructs an
//! order or enables any submit path from its own boundary code.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    rc::Rc,
    str::FromStr,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ahash::AHashMap;
use anyhow::{Result, anyhow};
use log::LevelFilter;
use nautilus_common::{
    actor::{DataActor, DataActorConfig, DataActorCore},
    enums::Environment,
    logging::logger::LoggerConfig,
    msgbus::{
        TypedHandler, subscribe_portfolio_snapshot, subscribe_position_events,
        unsubscribe_portfolio_snapshot, unsubscribe_position_events,
    },
    nautilus_actor,
};
use nautilus_live::{
    builder::LiveNodeBuilder,
    config::LiveNodeConfig,
    node::{LiveNode, LiveNodeHandle, NodeState},
};
use nautilus_model::{
    data::QuoteTick,
    enums::BarIntervalType,
    events::{PortfolioSnapshot, PositionEvent},
    identifiers::{AccountId, ActorId, ClientId, InstrumentId, PositionId, StrategyId},
    types::{Currency, Money},
};
use rust_decimal::Decimal;
use ustr::Ustr;
use zeroize::Zeroizing;

use crate::{
    bolt_v3_adapters::{BoltV3AdapterConfigs, BoltV3AdapterMappingError, map_bolt_v3_adapters},
    bolt_v3_client_registration::{
        BoltV3ClientRegistrationError, BoltV3RegistrationSummary, register_bolt_v3_clients,
    },
    bolt_v3_config::{LoadedBoltV3Config, LossGovernorBlock},
    bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence,
        BoltV3StrategyInputEvidenceSnapshot, JsonlBoltV3DecisionEvidenceWriter,
    },
    bolt_v3_live_canary_gate::{
        BoltV3LiveCanaryGateError, check_bolt_v3_live_canary_pre_consumption_gate,
        current_build_head_sha,
    },
    bolt_v3_loss_governor::{LossGovernorPolicy, LossSnapshot},
    bolt_v3_providers,
    bolt_v3_secrets::{
        BoltV3SecretError, ForbiddenEnvVarError, ResolvedBoltV3Secrets,
        check_no_forbidden_credential_env_vars, check_no_forbidden_credential_env_vars_with,
        resolve_bolt_v3_secrets, resolve_bolt_v3_secrets_with,
    },
    bolt_v3_strategy_registration::{
        BoltV3StrategyRegistrationError, register_bolt_v3_strategies_on_node_with_bindings,
    },
    bolt_v3_submit_admission::{BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionState},
    bolt_v3_tiny_canary_evidence::Phase8OperatorApprovalEnvelope,
    nt_runtime_capture::{self, NtRuntimeCaptureGuards, wire_nt_runtime_capture},
    secrets::SsmResolverSession,
};

pub struct BoltV3LiveNodeRuntime {
    node: LiveNode,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    redaction_values: Vec<Zeroizing<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3NoSubmitReferenceCacheEvidence {
    cached_instrument_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3NoSubmitReferenceQuote {
    pub data_client_id: String,
    pub instrument_id: String,
    pub ts_event_unix_nanos: u64,
    pub ts_init_unix_nanos: u64,
    pub captured_at_unix_nanos: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoltV3NoSubmitReferenceQuoteEvidence {
    pub quotes: Vec<BoltV3NoSubmitReferenceQuote>,
}

impl BoltV3NoSubmitReferenceQuoteEvidence {
    pub fn observed_at_unix_nanos(&self) -> Option<u64> {
        self.quotes
            .iter()
            .map(|quote| quote.captured_at_unix_nanos)
            .max()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NoSubmitReferenceQuoteSubscription {
    data_client_id: ClientId,
    instrument_id: InstrumentId,
}

#[derive(Debug, Clone)]
struct BoltV3NoSubmitReferenceQuoteProbeHandle {
    required: Vec<NoSubmitReferenceQuoteSubscription>,
    ambiguous_instrument_ids: BTreeSet<String>,
    quotes: Rc<RefCell<Vec<BoltV3NoSubmitReferenceQuote>>>,
    quote_notify: Rc<tokio::sync::Notify>,
}

impl BoltV3NoSubmitReferenceQuoteProbeHandle {
    fn new(loaded: &LoadedBoltV3Config) -> Self {
        let (required, ambiguous_instrument_ids) =
            no_submit_reference_quote_subscription_plan(loaded);
        Self {
            required,
            ambiguous_instrument_ids,
            quotes: Rc::new(RefCell::new(Vec::new())),
            quote_notify: Rc::new(tokio::sync::Notify::new()),
        }
    }

    fn has_all_required_quotes(&self) -> bool {
        if !self.ambiguous_instrument_ids.is_empty() {
            return false;
        }
        let quotes = self.quotes.borrow();
        self.required.iter().all(|required| {
            quotes.iter().any(|quote| {
                quote.data_client_id == required.data_client_id.to_string()
                    && quote.instrument_id == required.instrument_id.to_string()
            })
        })
    }

    fn ambiguity_error(&self) -> Option<String> {
        if self.ambiguous_instrument_ids.is_empty() {
            return None;
        }
        Some(
            "reference quote probe cannot distinguish multiple data clients for the same instrument_id; QuoteTick does not carry data_client_id"
                .to_string(),
        )
    }

    fn evidence(&self) -> BoltV3NoSubmitReferenceQuoteEvidence {
        BoltV3NoSubmitReferenceQuoteEvidence {
            quotes: self.quotes.borrow().clone(),
        }
    }

    fn record_quote(&self, quote: &QuoteTick, captured_at_unix_nanos: u64) {
        let quote_instrument_id = quote.instrument_id.to_string();
        if self.ambiguous_instrument_ids.contains(&quote_instrument_id) {
            return;
        }
        let mut matched_required = false;
        let mut quotes = self.quotes.borrow_mut();
        for required in &self.required {
            if quote.instrument_id == required.instrument_id {
                matched_required = true;
                quotes.push(BoltV3NoSubmitReferenceQuote {
                    data_client_id: required.data_client_id.to_string(),
                    instrument_id: required.instrument_id.to_string(),
                    ts_event_unix_nanos: quote.ts_event.as_u64(),
                    ts_init_unix_nanos: quote.ts_init.as_u64(),
                    captured_at_unix_nanos,
                });
            }
        }
        drop(quotes);
        if matched_required && self.has_all_required_quotes() {
            self.quote_notify.notify_one();
        }
    }

    async fn wait_for_all_required_quotes(&self) {
        while !self.has_all_required_quotes() {
            self.quote_notify.notified().await;
        }
    }
}

#[derive(Debug)]
struct BoltV3NoSubmitReferenceQuoteProbe {
    core: DataActorCore,
    handle: BoltV3NoSubmitReferenceQuoteProbeHandle,
}

nautilus_actor!(BoltV3NoSubmitReferenceQuoteProbe);

impl BoltV3NoSubmitReferenceQuoteProbe {
    fn new(handle: BoltV3NoSubmitReferenceQuoteProbeHandle, config: DataActorConfig) -> Self {
        Self {
            core: DataActorCore::new(config),
            handle,
        }
    }
}

impl DataActor for BoltV3NoSubmitReferenceQuoteProbe {
    fn on_start(&mut self) -> anyhow::Result<()> {
        for required in self.handle.required.clone() {
            self.subscribe_quotes(required.instrument_id, Some(required.data_client_id), None);
        }
        Ok(())
    }

    fn on_stop(&mut self) -> anyhow::Result<()> {
        for required in self.handle.required.clone() {
            self.unsubscribe_quotes(required.instrument_id, Some(required.data_client_id), None);
        }
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> anyhow::Result<()> {
        self.handle
            .record_quote(quote, self.timestamp_ns().as_u64());
        Ok(())
    }
}

fn no_submit_reference_quote_subscription_plan(
    loaded: &LoadedBoltV3Config,
) -> (Vec<NoSubmitReferenceQuoteSubscription>, BTreeSet<String>) {
    let mut seen = BTreeSet::new();
    let mut by_instrument: BTreeMap<String, String> = BTreeMap::new();
    let mut ambiguous_instrument_ids = BTreeSet::new();
    let mut subscriptions = Vec::new();
    for strategy in &loaded.strategies {
        for reference in strategy.config.reference_data.values() {
            let data_client_id = reference.data_client_id.to_string();
            let instrument_id = reference.instrument_id.to_string();
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
                subscriptions.push(NoSubmitReferenceQuoteSubscription {
                    data_client_id: reference.data_client_id,
                    instrument_id: reference.instrument_id,
                });
            }
        }
    }
    (subscriptions, ambiguous_instrument_ids)
}

impl BoltV3NoSubmitReferenceCacheEvidence {
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
}

impl BoltV3LiveNodeRuntime {
    fn new(
        node: LiveNode,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
        redaction_values: Vec<Zeroizing<String>>,
    ) -> Self {
        Self {
            node,
            submit_admission,
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

    pub fn registered_data_client_ids(&self) -> Vec<ClientId> {
        self.node.kernel().data_engine.borrow().registered_clients()
    }

    pub fn registered_exec_client_ids(&self) -> Vec<ClientId> {
        self.node.kernel().exec_engine.borrow().client_ids()
    }

    pub fn cached_instrument_ids(&self) -> Vec<String> {
        self.reference_cache_evidence().cached_instrument_ids
    }

    pub fn reference_cache_evidence(&self) -> BoltV3NoSubmitReferenceCacheEvidence {
        let cache = self.node.kernel().cache();
        let cache = cache.borrow();
        let cached_instrument_ids = cache
            .instrument_ids(None)
            .into_iter()
            .map(ToString::to_string)
            .collect();
        BoltV3NoSubmitReferenceCacheEvidence {
            cached_instrument_ids,
        }
    }

    pub fn redaction_values(&self) -> &[Zeroizing<String>] {
        &self.redaction_values
    }

    pub fn instance_id(&self) -> String {
        self.node.instance_id().to_string()
    }

    pub fn admitted_order_count(&self) -> u32 {
        self.submit_admission.admitted_order_count()
    }

    pub fn loss_governor_enabled(&self) -> bool {
        self.submit_admission.loss_governor_enabled()
    }
}

const BOLT_V3_LOSS_GOVERNOR_NT_SOURCE: &str = "nt_portfolio_and_position_events";

#[derive(Debug)]
struct BoltV3LossGovernorRuntimeGuards {
    position_events: Option<TypedHandler<PositionEvent>>,
    portfolio_snapshots: Option<TypedHandler<PortfolioSnapshot>>,
}

impl Drop for BoltV3LossGovernorRuntimeGuards {
    fn drop(&mut self) {
        if let Some(position_events) = self.position_events.take() {
            unsubscribe_position_events(
                nt_runtime_capture::position_events_pattern(),
                &position_events,
            );
        }
        if let Some(portfolio_snapshots) = self.portfolio_snapshots.take() {
            unsubscribe_portfolio_snapshot(
                nt_runtime_capture::portfolio_snapshots_pattern(),
                &portfolio_snapshots,
            );
        }
    }
}

#[derive(Debug)]
struct BoltV3LossGovernorRuntimeFeed {
    account_id: AccountId,
    rolling_window_ns: u64,
    position_pnls: HashMap<PositionId, TimedLossFact>,
    latest_daily_pnl: Option<TimedLossFact>,
    latest_rolling_pnl: Option<TimedLossFact>,
    latest_current_equity: Option<TimedLossFact>,
    peak_equity: Option<Decimal>,
    rolling_samples: VecDeque<(u64, Decimal)>,
}

#[derive(Debug, Clone, Copy)]
struct TimedLossFact {
    observed_at_ns: u64,
    value: Decimal,
}

impl BoltV3LossGovernorRuntimeFeed {
    fn new(account_id: AccountId, rolling_window_ns: u64) -> Self {
        Self {
            account_id,
            rolling_window_ns,
            position_pnls: HashMap::new(),
            latest_daily_pnl: None,
            latest_rolling_pnl: None,
            latest_current_equity: None,
            peak_equity: None,
            rolling_samples: VecDeque::new(),
        }
    }

    fn record_position_event(&mut self, event: &PositionEvent) -> Option<LossSnapshot> {
        if event.account_id() != self.account_id {
            return None;
        }
        match position_event_loss_fact(event) {
            PositionLossFact::Absolute {
                position_id,
                observed_at_ns,
                value,
            } => {
                self.position_pnls.insert(
                    position_id,
                    TimedLossFact {
                        observed_at_ns,
                        value,
                    },
                );
            }
            PositionLossFact::Adjustment {
                position_id,
                observed_at_ns,
                value,
            } => {
                if let Some(fact) = self.position_pnls.get(&position_id).copied() {
                    let value = value.map_or(fact.value, |delta| fact.value + delta);
                    self.position_pnls.insert(
                        position_id,
                        TimedLossFact {
                            observed_at_ns,
                            value,
                        },
                    );
                } else if let Some(value) = value {
                    self.position_pnls.insert(
                        position_id,
                        TimedLossFact {
                            observed_at_ns,
                            value,
                        },
                    );
                }
            }
            PositionLossFact::Closed { position_id } => {
                self.position_pnls.remove(&position_id);
            }
        }
        self.snapshot()
    }

    fn record_portfolio_snapshot(&mut self, snapshot: &PortfolioSnapshot) -> Option<LossSnapshot> {
        if snapshot.account_id != self.account_id {
            return None;
        }
        let observed_at_ns = snapshot.ts_event.as_u64();
        let Some(realized_pnl) = money_sum(&snapshot.realized_pnls) else {
            return Some(self.invalidate_portfolio_loss_facts(observed_at_ns));
        };
        let Some(unrealized_pnl) = money_sum(&snapshot.unrealized_pnls) else {
            return Some(self.invalidate_portfolio_loss_facts(observed_at_ns));
        };
        let Some(account_pnl) = combine_money_sums(realized_pnl, unrealized_pnl) else {
            return Some(self.invalidate_portfolio_loss_facts(observed_at_ns));
        };
        let Some(current_equity) = money_sum(&snapshot.total_equity) else {
            return Some(self.invalidate_portfolio_loss_facts(observed_at_ns));
        };
        let Some(snapshot_currency) =
            combine_money_currencies(account_pnl.currency, current_equity.currency)
        else {
            return Some(self.invalidate_portfolio_loss_facts(observed_at_ns));
        };
        if snapshot.base_currency.is_some_and(|base_currency| {
            snapshot_currency.is_some_and(|currency| currency != base_currency)
        }) {
            return Some(self.invalidate_portfolio_loss_facts(observed_at_ns));
        }
        let account_pnl = account_pnl.value;
        let current_equity = current_equity.value;
        self.latest_daily_pnl = Some(TimedLossFact {
            observed_at_ns,
            value: account_pnl,
        });
        self.latest_current_equity = Some(TimedLossFact {
            observed_at_ns,
            value: current_equity,
        });
        self.peak_equity = Some(
            self.peak_equity
                .map_or(current_equity, |peak| peak.max(current_equity)),
        );
        self.rolling_samples
            .push_back((observed_at_ns, account_pnl));
        let cutoff_ns = observed_at_ns.saturating_sub(self.rolling_window_ns);
        while self
            .rolling_samples
            .front()
            .is_some_and(|(sample_ns, _)| *sample_ns < cutoff_ns)
        {
            self.rolling_samples.pop_front();
        }
        self.latest_rolling_pnl = match (self.rolling_samples.front(), self.rolling_samples.back())
        {
            (Some((baseline_ns, baseline_pnl)), Some((latest_ns, _)))
                if baseline_ns < latest_ns =>
            {
                Some(TimedLossFact {
                    observed_at_ns,
                    value: account_pnl - *baseline_pnl,
                })
            }
            _ => None,
        };
        self.snapshot()
    }

    fn latest_per_trade_pnl(&self) -> Option<TimedLossFact> {
        self.position_pnls
            .values()
            .copied()
            .min_by(|left, right| left.value.cmp(&right.value))
    }

    fn invalidate_portfolio_loss_facts(&mut self, observed_at_ns: u64) -> LossSnapshot {
        self.latest_daily_pnl = None;
        self.latest_rolling_pnl = None;
        self.latest_current_equity = None;
        self.rolling_samples.clear();
        self.incomplete_portfolio_loss_snapshot(observed_at_ns)
    }

    fn incomplete_portfolio_loss_snapshot(&self, observed_at_ns: u64) -> LossSnapshot {
        let per_trade_pnl = self.latest_per_trade_pnl();
        LossSnapshot {
            source: BOLT_V3_LOSS_GOVERNOR_NT_SOURCE.to_string(),
            observed_at_ns,
            per_trade_pnl: per_trade_pnl.map(|fact| fact.value),
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        }
    }

    fn snapshot(&self) -> Option<LossSnapshot> {
        if self.latest_daily_pnl.is_none()
            && self.latest_rolling_pnl.is_none()
            && self.latest_current_equity.is_none()
        {
            return None;
        }
        let per_trade_pnl = self.latest_per_trade_pnl();
        let observed_at_ns = [
            per_trade_pnl,
            self.latest_daily_pnl,
            self.latest_rolling_pnl,
            self.latest_current_equity,
        ]
        .into_iter()
        .flatten()
        .map(|fact| fact.observed_at_ns)
        .min()?;
        Some(LossSnapshot {
            source: BOLT_V3_LOSS_GOVERNOR_NT_SOURCE.to_string(),
            observed_at_ns,
            per_trade_pnl: per_trade_pnl.map(|fact| fact.value),
            daily_pnl: self.latest_daily_pnl.map(|fact| fact.value),
            rolling_pnl: self.latest_rolling_pnl.map(|fact| fact.value),
            current_equity: self.latest_current_equity.map(|fact| fact.value),
            peak_equity: self.peak_equity,
        })
    }
}

enum PositionLossFact {
    Absolute {
        position_id: PositionId,
        observed_at_ns: u64,
        value: Decimal,
    },
    Adjustment {
        position_id: PositionId,
        observed_at_ns: u64,
        value: Option<Decimal>,
    },
    Closed {
        position_id: PositionId,
    },
}

fn position_event_loss_fact(event: &PositionEvent) -> PositionLossFact {
    match event {
        PositionEvent::PositionOpened(opened) => PositionLossFact::Absolute {
            position_id: opened.position_id,
            observed_at_ns: opened.ts_event.as_u64(),
            value: Decimal::ZERO,
        },
        PositionEvent::PositionChanged(changed) => PositionLossFact::Absolute {
            position_id: changed.position_id,
            observed_at_ns: changed.ts_event.as_u64(),
            value: changed
                .realized_pnl
                .map_or(Decimal::ZERO, |pnl| pnl.as_decimal())
                + changed.unrealized_pnl.as_decimal(),
        },
        PositionEvent::PositionClosed(closed) => PositionLossFact::Closed {
            position_id: closed.position_id,
        },
        PositionEvent::PositionAdjusted(adjusted) => PositionLossFact::Adjustment {
            position_id: adjusted.position_id,
            observed_at_ns: adjusted.ts_event.as_u64(),
            value: adjusted.pnl_change.map(|pnl| pnl.as_decimal()),
        },
    }
}

#[derive(Debug, Clone, Copy)]
struct LossMoneySum {
    currency: Option<Currency>,
    value: Decimal,
}

fn money_sum(values: &[Money]) -> Option<LossMoneySum> {
    let mut values = values.iter();
    let first = values.next()?;
    let mut currency = Some(first.currency);
    let mut total = first.as_decimal();
    for value in values {
        currency = combine_money_currencies(currency, Some(value.currency))?;
        total += value.as_decimal();
    }
    Some(LossMoneySum {
        currency,
        value: total,
    })
}

fn combine_money_sums(left: LossMoneySum, right: LossMoneySum) -> Option<LossMoneySum> {
    Some(LossMoneySum {
        currency: combine_money_currencies(left.currency, right.currency)?,
        value: left.value + right.value,
    })
}

fn combine_money_currencies(
    left: Option<Currency>,
    right: Option<Currency>,
) -> Option<Option<Currency>> {
    match (left, right) {
        (Some(left), Some(right)) if left != right => None,
        (Some(currency), None) | (None, Some(currency)) | (Some(currency), Some(_)) => {
            Some(Some(currency))
        }
        (None, None) => Some(None),
    }
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
    LossGovernorConfig(anyhow::Error),
    BuilderConstruction(BoltV3LiveNodeBuilderError),
    ClientRegistration(BoltV3ClientRegistrationError),
    StrategyRegistration(BoltV3StrategyRegistrationError),
    Build(anyhow::Error),
    /// The live canary gate rejected entry to NT's runner loop before
    /// `LiveNode::run` was invoked. This variant wraps the specific
    /// fail-closed reason from [`BoltV3LiveCanaryGateError`].
    LiveCanaryGate(BoltV3LiveCanaryGateError),
    /// The live runner entrypoint passed the pre-consumption gate but
    /// could not atomically create the operator-approval consumption
    /// proof before arming submit admission.
    OperatorApprovalConsumption(anyhow::Error),
    /// The validated live canary gate report could not arm the shared
    /// submit-admission state before `LiveNode::run` was invoked.
    SubmitAdmission(BoltV3SubmitAdmissionError),
    /// NT returned an error from `LiveNode::run` after the live canary
    /// gate accepted the loaded config and readiness report.
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
    NoSubmitStartTimeout {
        timeout_secs: u64,
    },
    NoSubmitStartTimeoutOverflow,
    NoSubmitStartIncomplete,
    NoSubmitExecutionAccountsMissing {
        client_venues: Vec<String>,
    },
    NoSubmitReferenceProbeSetup(anyhow::Error),
    NoSubmitReferenceProbeFailed {
        reason: String,
    },
    NoSubmitStartFailed(anyhow::Error),
    NoSubmitStopTimeout {
        timeout_secs: u64,
    },
    NoSubmitStopTimeoutOverflow,
    NoSubmitStopFailed(anyhow::Error),
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
            BoltV3LiveNodeError::LossGovernorConfig(error) => {
                write!(f, "bolt-v3 loss-governor config mapping failed: {error}")
            }
            BoltV3LiveNodeError::BuilderConstruction(error) => write!(f, "{error}"),
            BoltV3LiveNodeError::ClientRegistration(error) => {
                write!(f, "bolt-v3 client registration failed: {error}")
            }
            BoltV3LiveNodeError::StrategyRegistration(error) => {
                write!(f, "bolt-v3 strategy registration failed: {error}")
            }
            BoltV3LiveNodeError::Build(error) => write!(f, "LiveNode build failed: {error}"),
            BoltV3LiveNodeError::LiveCanaryGate(error) => {
                write!(
                    f,
                    "bolt-v3 live canary gate rejected runtime start: {error}"
                )
            }
            BoltV3LiveNodeError::OperatorApprovalConsumption(error) => {
                write!(
                    f,
                    "bolt-v3 live canary approval consumption failed before runtime start: {error}"
                )
            }
            BoltV3LiveNodeError::SubmitAdmission(error) => {
                write!(
                    f,
                    "bolt-v3 submit admission rejected runtime start: {error}"
                )
            }
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
            BoltV3LiveNodeError::NoSubmitStartTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 no-submit controlled-start exceeded configured \
                 live-node timeout bounds ({timeout_secs}s)"
            ),
            BoltV3LiveNodeError::NoSubmitStartTimeoutOverflow => write!(
                f,
                "bolt-v3 no-submit controlled-start timeout sum overflowed \
                 config-owned nautilus timeout fields"
            ),
            BoltV3LiveNodeError::NoSubmitStartIncomplete => write!(
                f,
                "bolt-v3 no-submit controlled-run exited before NT reached Running \
                 with required startup evidence"
            ),
            BoltV3LiveNodeError::NoSubmitExecutionAccountsMissing { client_venues } => write!(
                f,
                "bolt-v3 no-submit controlled-run reached NT Running but required execution \
                 account evidence was absent from NT cache for: {}",
                client_venues.join(", ")
            ),
            BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(error) => write!(
                f,
                "bolt-v3 no-submit reference quote probe setup failed: {error}"
            ),
            BoltV3LiveNodeError::NoSubmitReferenceProbeFailed { reason } => write!(
                f,
                "bolt-v3 no-submit controlled-run reached NT Running but live reference quote evidence was not observed; engine connectivity cannot be treated as proven: {reason}"
            ),
            BoltV3LiveNodeError::NoSubmitStartFailed(error) => {
                write!(f, "bolt-v3 no-submit controlled-start failed: {error}")
            }
            BoltV3LiveNodeError::NoSubmitStopTimeout { timeout_secs } => write!(
                f,
                "bolt-v3 no-submit controlled-stop exceeded configured \
                 live-node timeout bounds ({timeout_secs}s)"
            ),
            BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow => write!(
                f,
                "bolt-v3 no-submit controlled-stop timeout sum overflowed \
                 config-owned nautilus timeout fields"
            ),
            BoltV3LiveNodeError::NoSubmitStopFailed(error) => {
                write!(f, "bolt-v3 no-submit controlled-stop failed: {error}")
            }
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
            BoltV3LiveNodeError::LossGovernorConfig(error) => Some(error.as_ref()),
            BoltV3LiveNodeError::BuilderConstruction(error) => Some(error),
            BoltV3LiveNodeError::ClientRegistration(error) => Some(error),
            BoltV3LiveNodeError::StrategyRegistration(error) => Some(error),
            BoltV3LiveNodeError::Build(error) => error.source(),
            BoltV3LiveNodeError::LiveCanaryGate(error) => Some(error),
            BoltV3LiveNodeError::OperatorApprovalConsumption(error) => Some(error.as_ref()),
            BoltV3LiveNodeError::SubmitAdmission(error) => Some(error),
            BoltV3LiveNodeError::Run(error) => error.source(),
            BoltV3LiveNodeError::RuntimeCaptureWire(error)
            | BoltV3LiveNodeError::RuntimeCaptureShutdown(error) => error.source(),
            BoltV3LiveNodeError::RunAndRuntimeCaptureShutdown { run_error, .. } => {
                Some(run_error.as_ref())
            }
            BoltV3LiveNodeError::ConnectTimeout { .. }
            | BoltV3LiveNodeError::ConnectIncomplete
            | BoltV3LiveNodeError::DisconnectTimeout { .. }
            | BoltV3LiveNodeError::NoSubmitStartTimeout { .. }
            | BoltV3LiveNodeError::NoSubmitStartTimeoutOverflow
            | BoltV3LiveNodeError::NoSubmitStartIncomplete
            | BoltV3LiveNodeError::NoSubmitExecutionAccountsMissing { .. }
            | BoltV3LiveNodeError::NoSubmitReferenceProbeFailed { .. }
            | BoltV3LiveNodeError::NoSubmitStopTimeout { .. }
            | BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow => None,
            BoltV3LiveNodeError::DisconnectFailed(error)
            | BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(error)
            | BoltV3LiveNodeError::NoSubmitStartFailed(error)
            | BoltV3LiveNodeError::NoSubmitStopFailed(error) => Some(error.as_ref()),
        }
    }
}

pub fn build_bolt_v3_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let resolved = resolve_bolt_v3_live_node_secrets(loaded)?;
    let adapters =
        map_bolt_v3_adapters(loaded, &resolved).map_err(BoltV3LiveNodeError::AdapterMapping)?;
    let (runtime, _summary) = build_live_node_with_clients(loaded, &resolved, adapters)?;
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

pub fn build_bolt_v3_no_submit_live_node(
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3LiveNodeRuntime, BoltV3LiveNodeError> {
    let resolved = resolve_bolt_v3_live_node_secrets(loaded)?;
    let adapters = no_submit_transport_adapter_configs(loaded, &resolved)?;
    let no_submit_loaded = no_submit_transport_loaded_config(loaded);
    let (runtime, _summary) = build_live_node_with_clients(&no_submit_loaded, &resolved, adapters)?;
    Ok(runtime)
}

fn no_submit_transport_adapter_configs(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
) -> Result<BoltV3AdapterConfigs, BoltV3LiveNodeError> {
    map_bolt_v3_adapters(loaded, resolved).map_err(BoltV3LiveNodeError::AdapterMapping)
}

fn no_submit_transport_loaded_config(loaded: &LoadedBoltV3Config) -> LoadedBoltV3Config {
    let mut no_submit_loaded = loaded.clone();
    no_submit_loaded.strategies.clear();
    no_submit_loaded
}

/// Single bolt-v3 entrypoint for entering NT's runner loop.
///
/// The caller builds the `LiveNode` separately, then this function checks
/// the loaded config's `[live_canary]` section and referenced no-submit
/// readiness report before entering the NT runner loop. Production callers
/// must use this wrapper rather than invoking the NT runner method directly.
/// If the gate rejects, NT's runner loop is never entered.
pub async fn run_bolt_v3_live_node(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let gate_report = check_bolt_v3_live_canary_pre_consumption_gate(loaded)
        .await
        .map_err(BoltV3LiveNodeError::LiveCanaryGate)?;
    consume_bolt_v3_live_runner_approval(loaded)
        .map_err(BoltV3LiveNodeError::OperatorApprovalConsumption)?;
    runtime
        .submit_admission
        .arm(gate_report)
        .map_err(BoltV3LiveNodeError::SubmitAdmission)?;
    let _loss_governor_guards = wire_bolt_v3_loss_governor_runtime(runtime, loaded);
    let node = &mut runtime.node;
    let node_handle = node.handle();
    let mut capture_guards = wire_bolt_v3_runtime_capture(node, node_handle, loaded)
        .map_err(BoltV3LiveNodeError::RuntimeCaptureWire)?;
    let mut capture_failure_receiver = capture_guards.take_failure_receiver();

    let run_result = {
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
    let shutdown_result = capture_guards.shutdown().await;

    classify_live_node_run_and_capture_shutdown(run_result, shutdown_result)
}

pub fn consume_bolt_v3_live_runner_approval(
    loaded: &LoadedBoltV3Config,
) -> Result<(), anyhow::Error> {
    let live_canary = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing `[live_canary]` config"))?;
    let operator_evidence = live_canary
        .operator_evidence
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("missing `[live_canary.operator_evidence]` config"))?;
    let current_head_sha = current_build_head_sha()
        .ok_or_else(|| anyhow::anyhow!("bolt-v3 build head_sha is unavailable or invalid"))?;
    let current_root_toml_sha256 = Phase8OperatorApprovalEnvelope::sha256_file(&loaded.root_path)?;
    let current_unix_secs = current_unix_seconds_i64()?;
    let envelope = Phase8OperatorApprovalEnvelope {
        head_sha: operator_evidence.head_sha.clone(),
        root_toml_path: loaded.root_path.to_string_lossy().to_string(),
        root_toml_sha256: current_root_toml_sha256.clone(),
        approval_envelope_sha256: operator_evidence.approval_envelope_sha256.clone(),
        ssm_manifest_path: operator_evidence.ssm_manifest_path.clone(),
        ssm_manifest_sha256: operator_evidence.ssm_manifest_sha256.clone(),
        strategy_input_evidence_path: operator_evidence.strategy_input_evidence_path.clone(),
        strategy_input_evidence_sha256: operator_evidence.strategy_input_evidence_sha256.clone(),
        financial_envelope_path: operator_evidence.financial_envelope_path.clone(),
        financial_envelope_sha256: operator_evidence.financial_envelope_sha256.clone(),
        pre_run_state_path: operator_evidence.pre_run_state_path.clone(),
        pre_run_state_sha256: operator_evidence.pre_run_state_sha256.clone(),
        abort_plan_path: operator_evidence.abort_plan_path.clone(),
        abort_plan_sha256: operator_evidence.abort_plan_sha256.clone(),
        operator_approval_id: live_canary.approval_id.clone(),
        approval_not_before_unix_secs: operator_evidence.approval_not_before_unix_seconds,
        approval_not_after_unix_secs: operator_evidence.approval_not_after_unix_seconds,
        approval_nonce_path: operator_evidence.approval_nonce_path.clone(),
        approval_nonce_sha256: operator_evidence.approval_nonce_sha256.clone(),
        approval_consumption_path: operator_evidence.approval_consumption_path.clone(),
        canary_evidence_path: operator_evidence.canary_evidence_path.clone(),
        strategy_cancel_path: operator_evidence.strategy_cancel_path.clone(),
    };

    envelope.validate_and_consume_against(
        current_head_sha,
        &current_root_toml_sha256,
        &live_canary.approval_id,
        loaded,
        current_unix_secs,
    )
}

fn current_unix_seconds_i64() -> Result<i64, anyhow::Error> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|source| anyhow::anyhow!("system time is before UNIX_EPOCH: {source}"))?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|source| anyhow::anyhow!("current unix seconds exceeds i64: {source}"))
}

pub async fn controlled_no_submit_readiness<F>(
    runtime: &mut BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
    mut reference_readiness: F,
) -> (
    Result<(), BoltV3LiveNodeError>,
    Result<(), String>,
    Result<(), BoltV3LiveNodeError>,
)
where
    F: FnMut(&BoltV3LiveNodeRuntime, &BoltV3NoSubmitReferenceQuoteEvidence) -> Result<(), String>,
{
    let (run, reference_quote_evidence, reference_quote_probe, stop) =
        run_bolt_v3_no_submit_readiness_until_observed(&mut runtime.node, loaded).await;
    let execution_accounts = match run {
        Ok(()) => no_submit_required_execution_accounts_registered(runtime, loaded),
        Err(error) => Err(error),
    };
    let connect = no_submit_controlled_connect_result(execution_accounts, &reference_quote_probe);
    let reference = if connect.is_ok() {
        reference_quote_probe.and_then(|()| reference_readiness(runtime, &reference_quote_evidence))
    } else {
        Err("controlled connect failed".to_string())
    };
    (connect, reference, stop)
}

fn no_submit_controlled_connect_result(
    execution_accounts: Result<(), BoltV3LiveNodeError>,
    reference_quote_probe: &Result<(), String>,
) -> Result<(), BoltV3LiveNodeError> {
    execution_accounts?;
    match reference_quote_probe {
        Ok(()) => Ok(()),
        Err(reason) => Err(BoltV3LiveNodeError::NoSubmitReferenceProbeFailed {
            reason: reason.clone(),
        }),
    }
}

async fn run_bolt_v3_no_submit_readiness_until_observed(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> (
    Result<(), BoltV3LiveNodeError>,
    BoltV3NoSubmitReferenceQuoteEvidence,
    Result<(), String>,
    Result<(), BoltV3LiveNodeError>,
) {
    let reference_quote_probe = match install_no_submit_reference_quote_probe(node, loaded) {
        Ok(probe) => probe,
        Err(error) => {
            return (
                Err(error),
                BoltV3NoSubmitReferenceQuoteEvidence { quotes: Vec::new() },
                Err("reference quote probe setup failed".to_string()),
                Err(BoltV3LiveNodeError::NoSubmitStopFailed(anyhow::anyhow!(
                    "no-submit runner was not started because reference quote probe setup failed"
                ))),
            );
        }
    };
    let timeout_secs = match no_submit_start_timeout_secs(loaded) {
        Ok(timeout_secs) => timeout_secs,
        Err(error) => {
            return (
                Err(error),
                reference_quote_probe.evidence(),
                Err(
                    "reference quote probe was not observed because start timeout overflowed"
                        .to_string(),
                ),
                Err(BoltV3LiveNodeError::NoSubmitStopFailed(anyhow::anyhow!(
                    "no-submit runner was not started because the configured start timeout overflowed"
                ))),
            );
        }
    };
    let stop_timeout_secs = match no_submit_stop_timeout_secs(loaded) {
        Ok(timeout_secs) => timeout_secs,
        Err(_) => {
            return (
                Err(BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow),
                reference_quote_probe.evidence(),
                Err(
                    "reference quote probe was not observed because stop timeout overflowed"
                        .to_string(),
                ),
                Err(BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow),
            );
        }
    };
    let node_handle = node.handle();
    let run_future = node.run();
    tokio::pin!(run_future);

    let connect = tokio::select! {
        result = &mut run_future => {
            let connect = match result {
                Ok(()) => Err(BoltV3LiveNodeError::NoSubmitStartIncomplete),
                Err(error) => Err(BoltV3LiveNodeError::NoSubmitStartFailed(error)),
            };
            return (
                connect,
                reference_quote_probe.evidence(),
                Err("reference quote probe was not observed before runner exit".to_string()),
                Ok(()),
            );
        }
        result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            await_no_submit_running(&node_handle, loaded),
        ) => {
            match result {
                Ok(()) => Ok(()),
                Err(_) => {
                    return (
                        Err(BoltV3LiveNodeError::NoSubmitStartTimeout { timeout_secs }),
                        reference_quote_probe.evidence(),
                        Err("reference quote probe was not observed because no-submit runner did not reach Running".to_string()),
                        Err(BoltV3LiveNodeError::NoSubmitStopFailed(anyhow::anyhow!(
                            "no-submit runner did not reach Running before the configured start timeout; NT does not observe stop signals during startup"
                        ))),
                    );
                }
            }
        }
    };

    let reference_probe = tokio::select! {
        result = &mut run_future => {
            let stop = match result {
                Ok(()) => Ok(()),
                Err(error) => Err(BoltV3LiveNodeError::NoSubmitStopFailed(error)),
            };
            return (
                connect,
                reference_quote_probe.evidence(),
                Err("reference quote probe was not observed before runner exit".to_string()),
                stop,
            );
        }
        result = await_no_submit_reference_quote_probe(&reference_quote_probe, loaded) => result,
    };
    let reference_quote_evidence = reference_quote_probe.evidence();
    node_handle.stop();
    let stop =
        match tokio::time::timeout(Duration::from_secs(stop_timeout_secs), &mut run_future).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(BoltV3LiveNodeError::NoSubmitStopFailed(error)),
            Err(_) => Err(BoltV3LiveNodeError::NoSubmitStopTimeout {
                timeout_secs: stop_timeout_secs,
            }),
        };
    (connect, reference_quote_evidence, reference_probe, stop)
}

fn install_no_submit_reference_quote_probe(
    node: &mut LiveNode,
    loaded: &LoadedBoltV3Config,
) -> Result<BoltV3NoSubmitReferenceQuoteProbeHandle, BoltV3LiveNodeError> {
    let handle = BoltV3NoSubmitReferenceQuoteProbeHandle::new(loaded);
    if let Some(message) = handle.ambiguity_error() {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
            anyhow::anyhow!(message),
        ));
    }
    let config = no_submit_reference_quote_probe_config(loaded)?;
    node.add_actor(BoltV3NoSubmitReferenceQuoteProbe::new(
        handle.clone(),
        config,
    ))
    .map_err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup)?;
    Ok(handle)
}

fn no_submit_reference_quote_probe_config(
    loaded: &LoadedBoltV3Config,
) -> Result<DataActorConfig, BoltV3LiveNodeError> {
    let live_canary = loaded.root.live_canary.as_ref().ok_or_else(|| {
        BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(anyhow::anyhow!(
            "bolt-v3 no-submit reference quote probe requires [live_canary]"
        ))
    })?;
    let actor_id_value = live_canary.reference_quote_probe_actor_id.as_str();
    if actor_id_value.trim().is_empty() || actor_id_value.trim() != actor_id_value {
        return Err(BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(
            anyhow::anyhow!(
                "[live_canary].reference_quote_probe_actor_id must be non-empty without surrounding whitespace"
            ),
        ));
    }
    let actor_id = ActorId::new_checked(actor_id_value).map_err(|error| {
        BoltV3LiveNodeError::NoSubmitReferenceProbeSetup(anyhow::anyhow!(
            "[live_canary].reference_quote_probe_actor_id is invalid: {error}"
        ))
    })?;

    Ok(DataActorConfig {
        actor_id: Some(actor_id),
        log_events: live_canary.reference_quote_probe_log_events,
        log_commands: live_canary.reference_quote_probe_log_commands,
    })
}

async fn await_no_submit_reference_quote_probe(
    probe: &BoltV3NoSubmitReferenceQuoteProbeHandle,
    loaded: &LoadedBoltV3Config,
) -> Result<(), String> {
    let timeout_secs = loaded
        .root
        .live_canary
        .as_ref()
        .ok_or_else(|| "reference quote probe wait requires `[live_canary]`".to_string())?
        .reference_quote_wait_timeout_seconds;
    if timeout_secs == 0 {
        return Err(
            "[live_canary].reference_quote_wait_timeout_seconds must be a positive integer"
                .to_string(),
        );
    }
    tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        probe.wait_for_all_required_quotes().await;
    })
    .await
    .map_err(|_| {
        format!(
            "reference quote probe did not observe all configured reference_data quotes within [live_canary].reference_quote_wait_timeout_seconds={timeout_secs}"
        )
    })
}

async fn await_no_submit_running(node_handle: &LiveNodeHandle, loaded: &LoadedBoltV3Config) {
    let poll_interval = Duration::from_millis(
        loaded
            .root
            .persistence
            .runtime_capture_start_poll_interval_ms,
    );
    while node_handle.state() != NodeState::Running {
        tokio::time::sleep(poll_interval).await;
    }
}

fn no_submit_required_execution_accounts_registered(
    runtime: &BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
) -> Result<(), BoltV3LiveNodeError> {
    let cache = runtime.node.kernel().cache();
    let cache = cache.borrow();
    let client_venues = loaded
        .root
        .clients
        .iter()
        .filter(|(_, client)| client.execution.is_some())
        .filter(|(_, client)| cache.account_for_venue(&client.venue).is_none())
        .map(|(client_key, client)| format!("clients.{client_key} ({})", client.venue))
        .collect::<Vec<_>>();

    if client_venues.is_empty() {
        Ok(())
    } else {
        Err(BoltV3LiveNodeError::NoSubmitExecutionAccountsMissing { client_venues })
    }
}

fn no_submit_start_timeout_secs(loaded: &LoadedBoltV3Config) -> Result<u64, BoltV3LiveNodeError> {
    loaded
        .root
        .nautilus
        .timeout_connection_secs
        .checked_add(loaded.root.nautilus.timeout_reconciliation_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_portfolio_secs))
        .ok_or(BoltV3LiveNodeError::NoSubmitStartTimeoutOverflow)
}

fn no_submit_stop_timeout_secs(loaded: &LoadedBoltV3Config) -> Result<u64, BoltV3LiveNodeError> {
    loaded
        .root
        .nautilus
        .timeout_disconnection_secs
        .checked_add(loaded.root.nautilus.delay_post_stop_secs)
        .and_then(|sum| sum.checked_add(loaded.root.nautilus.timeout_shutdown_secs))
        .ok_or(BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow)
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
    check_no_forbidden_credential_env_vars_with(&loaded.root, env_is_set)
        .map_err(BoltV3LiveNodeError::ForbiddenEnv)?;
    let resolved = resolve_bolt_v3_secrets_with(loaded, resolver)
        .map_err(BoltV3LiveNodeError::SecretResolution)?;
    let adapters =
        map_bolt_v3_adapters(loaded, &resolved).map_err(BoltV3LiveNodeError::AdapterMapping)?;
    build_live_node_with_clients(loaded, &resolved, adapters)
}

fn build_live_node_with_clients(
    loaded: &LoadedBoltV3Config,
    resolved: &ResolvedBoltV3Secrets,
    adapters: BoltV3AdapterConfigs,
) -> Result<(BoltV3LiveNodeRuntime, BoltV3RegistrationSummary), BoltV3LiveNodeError> {
    let decision_evidence: Arc<dyn BoltV3DecisionEvidenceWriter> = if loaded.strategies.is_empty() {
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
    };
    let loss_governor_policy =
        configured_loss_governor_policy(loaded).map_err(BoltV3LiveNodeError::LossGovernorConfig)?;
    let submit_admission = Arc::new(match loss_governor_policy {
        Some(policy) => BoltV3SubmitAdmissionState::new_unarmed_with_loss_governor(
            decision_evidence.clone(),
            policy,
        ),
        None => BoltV3SubmitAdmissionState::new_unarmed(decision_evidence.clone()),
    });
    let builder =
        make_bolt_v3_live_node_builder(loaded).map_err(BoltV3LiveNodeError::BuilderConstruction)?;
    let (builder, summary) = register_bolt_v3_clients(builder, adapters)
        .map_err(BoltV3LiveNodeError::ClientRegistration)?;
    let mut node = builder.build().map_err(BoltV3LiveNodeError::Build)?;
    let strategy_summary = register_bolt_v3_strategies_on_node_with_bindings(
        &mut node,
        loaded,
        resolved,
        crate::bolt_v3_archetypes::runtime_bindings(),
        submit_admission.clone(),
        decision_evidence,
    )
    .map_err(BoltV3LiveNodeError::StrategyRegistration)?;
    for strategy in &strategy_summary.registered {
        log::info!(
            "bolt-v3 registered strategy: strategy_instance_id={} strategy_archetype={} nt_strategy_id={}",
            strategy.strategy_instance_id,
            strategy.strategy_archetype.as_str(),
            strategy.registered_strategy_id
        );
    }
    Ok((
        BoltV3LiveNodeRuntime::new(node, submit_admission, resolved.redaction_values()),
        summary,
    ))
}

fn configured_loss_governor_policy(
    loaded: &LoadedBoltV3Config,
) -> Result<Option<LossGovernorPolicy>> {
    let validation_messages = crate::bolt_v3_validate::validate_root_only(&loaded.root);
    if !validation_messages.is_empty() {
        return Err(
            crate::bolt_v3_validate::BoltV3ValidationError::new(validation_messages).into(),
        );
    }
    let Some(block) = loaded
        .root
        .risk
        .loss_governor
        .as_ref()
        .filter(|block| block.enabled)
    else {
        return Ok(None);
    };
    loss_governor_policy_from_validated_block(block).map(Some)
}

fn loss_governor_policy_from_validated_block(
    block: &LossGovernorBlock,
) -> Result<LossGovernorPolicy> {
    Ok(LossGovernorPolicy {
        max_snapshot_age_ns: block.max_snapshot_age_ns,
        max_per_trade_loss: loss_governor_validated_decimal(
            "risk.loss_governor.max_per_trade_loss",
            block.max_per_trade_loss.as_deref(),
        )?,
        max_daily_loss: loss_governor_validated_decimal(
            "risk.loss_governor.max_daily_loss",
            block.max_daily_loss.as_deref(),
        )?,
        max_rolling_loss: loss_governor_validated_decimal(
            "risk.loss_governor.max_rolling_loss",
            block.max_rolling_loss.as_deref(),
        )?,
        max_drawdown: loss_governor_validated_decimal(
            "risk.loss_governor.max_drawdown",
            block.max_drawdown.as_deref(),
        )?,
    })
}

fn loss_governor_validated_decimal(label: &str, value: Option<&str>) -> Result<Decimal> {
    let raw = value.ok_or_else(|| anyhow!("{label} missing after root config validation"))?;
    let value = crate::bolt_v3_validate::parse_decimal_string(raw).map_err(|reason| {
        anyhow!("{label} failed to parse after root config validation ({reason}): `{raw}`")
    })?;
    Ok(value)
}

fn wire_bolt_v3_loss_governor_runtime(
    runtime: &BoltV3LiveNodeRuntime,
    loaded: &LoadedBoltV3Config,
) -> BoltV3LossGovernorRuntimeGuards {
    let Some(block) = loaded
        .root
        .risk
        .loss_governor
        .as_ref()
        .filter(|block| block.enabled)
    else {
        return BoltV3LossGovernorRuntimeGuards {
            position_events: None,
            portfolio_snapshots: None,
        };
    };

    let feed = Arc::new(Mutex::new(BoltV3LossGovernorRuntimeFeed::new(
        block.account_id,
        block.rolling_window_ns,
    )));
    let submit_for_positions = runtime.submit_admission.clone();
    let position_feed = feed.clone();
    let position_events = TypedHandler::from(move |event: &PositionEvent| {
        let mut feed = position_feed
            .lock()
            .expect("loss governor runtime feed mutex should not be poisoned");
        if let Some(snapshot) = feed.record_position_event(event) {
            submit_for_positions.update_loss_snapshot(snapshot);
        }
    });
    subscribe_position_events(
        nt_runtime_capture::position_events_pattern(),
        position_events.clone(),
        None,
    );

    let submit_for_portfolio = runtime.submit_admission.clone();
    let portfolio_feed = feed.clone();
    let portfolio_snapshots = TypedHandler::from(move |snapshot: &PortfolioSnapshot| {
        let mut feed = portfolio_feed
            .lock()
            .expect("loss governor runtime feed mutex should not be poisoned");
        if let Some(loss_snapshot) = feed.record_portfolio_snapshot(snapshot) {
            submit_for_portfolio.update_loss_snapshot(loss_snapshot);
        }
    });
    subscribe_portfolio_snapshot(
        nt_runtime_capture::portfolio_snapshots_pattern(),
        portfolio_snapshots.clone(),
        None,
    );

    BoltV3LossGovernorRuntimeGuards {
        position_events: Some(position_events),
        portfolio_snapshots: Some(portfolio_snapshots),
    }
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
        bypass: loaded.root.risk.nautilus.bypass,
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
    use crate::bolt_v3_config::{BoltV3RootConfig, ReferenceDataBlock};
    use nautilus_core::UUID4;
    use nautilus_model::enums::{AccountType, OrderSide, PositionAdjustmentType, PositionSide};
    use nautilus_model::events::{PositionAdjusted, PositionChanged, PositionClosed};
    use nautilus_model::identifiers::{
        AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId, TraderId,
    };
    use nautilus_model::types::{Currency, Money, Price, Quantity};

    fn fixture_loaded_config() -> LoadedBoltV3Config {
        let root_text = include_str!("../tests/fixtures/bolt_v3/root.toml");
        let root: BoltV3RootConfig = toml::from_str(root_text).unwrap();
        LoadedBoltV3Config {
            root_path: std::path::PathBuf::from("tests/fixtures/bolt_v3/root.toml"),
            config_bundle_checksum: String::new(),
            root,
            strategies: Vec::new(),
        }
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
    fn no_submit_controlled_connect_rejects_unobserved_reference_probe() {
        let error = no_submit_controlled_connect_result(
            Ok(()),
            &Err("reference quote probe timed out".to_string()),
        )
        .expect_err("controlled connect must not be satisfied when reference quote evidence was not observed");

        match error {
            BoltV3LiveNodeError::NoSubmitReferenceProbeFailed { reason } => {
                assert_eq!(reason, "reference quote probe timed out");
            }
            other => panic!("expected NoSubmitReferenceProbeFailed, got {other}"),
        }
    }

    #[test]
    fn no_submit_transport_config_preserves_identity_but_removes_strategy_instances() {
        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        assert!(
            !loaded.strategies.is_empty(),
            "fixture must include strategy config to prove no-submit transport strips it"
        );

        let no_submit_loaded = no_submit_transport_loaded_config(&loaded);

        assert!(
            no_submit_loaded.strategies.is_empty(),
            "no-submit transport runtime must not register strategy actors"
        );
        assert_eq!(no_submit_loaded.root_path, loaded.root_path);
        assert_eq!(
            no_submit_loaded.config_bundle_checksum,
            loaded.config_bundle_checksum
        );
        assert_eq!(
            no_submit_loaded.root.strategy_files,
            loaded.root.strategy_files
        );
        assert!(
            !loaded.strategies.is_empty(),
            "helper must not mutate the caller's loaded config"
        );
    }

    #[test]
    fn loss_governor_feed_builds_snapshots_from_nt_portfolio_and_position_events() {
        let account_id = AccountId::from("POLYMARKET-001");
        let mut feed = BoltV3LossGovernorRuntimeFeed::new(account_id, 1_000);

        let first = feed
            .record_portfolio_snapshot(&portfolio_snapshot(
                account_id,
                100,
                Decimal::new(-1, 0),
                Decimal::new(-2, 0),
                Decimal::new(100, 0),
            ))
            .expect("first matching portfolio snapshot should produce loss snapshot");
        assert_eq!(first.observed_at_ns, 100);
        assert_eq!(first.per_trade_pnl, None);
        assert_eq!(first.daily_pnl, Some(Decimal::new(-3, 0)));
        assert_eq!(first.rolling_pnl, None);
        assert_eq!(first.current_equity, Some(Decimal::new(100, 0)));
        assert_eq!(first.peak_equity, Some(Decimal::new(100, 0)));

        let with_position = feed
            .record_position_event(&position_changed_event(
                account_id,
                150,
                Decimal::new(-4, 0),
                Decimal::new(-2, 0),
            ))
            .expect("position event after portfolio snapshot should refresh loss snapshot");
        assert_eq!(with_position.observed_at_ns, 100);
        assert_eq!(with_position.per_trade_pnl, Some(Decimal::new(-6, 0)));
        assert_eq!(with_position.daily_pnl, Some(Decimal::new(-3, 0)));
        assert_eq!(with_position.rolling_pnl, None);

        let second = feed
            .record_portfolio_snapshot(&portfolio_snapshot(
                account_id,
                200,
                Decimal::new(-5, 0),
                Decimal::new(-6, 0),
                Decimal::new(90, 0),
            ))
            .expect("second matching portfolio snapshot should update rolling and drawdown facts");
        assert_eq!(second.observed_at_ns, 150);
        assert_eq!(second.per_trade_pnl, Some(Decimal::new(-6, 0)));
        assert_eq!(second.daily_pnl, Some(Decimal::new(-11, 0)));
        assert_eq!(second.rolling_pnl, Some(Decimal::new(-8, 0)));
        assert_eq!(second.current_equity, Some(Decimal::new(90, 0)));
        assert_eq!(second.peak_equity, Some(Decimal::new(100, 0)));
    }

    #[test]
    fn loss_governor_feed_requires_rolling_baseline_inside_configured_window() {
        let account_id = AccountId::from("POLYMARKET-001");
        let mut feed = BoltV3LossGovernorRuntimeFeed::new(account_id, 1_000);

        feed.record_portfolio_snapshot(&portfolio_snapshot(
            account_id,
            100,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::new(100, 0),
        ))
        .expect("initial portfolio snapshot should seed rolling baseline");

        let second = feed
            .record_portfolio_snapshot(&portfolio_snapshot(
                account_id,
                1_200,
                Decimal::new(-10, 0),
                Decimal::ZERO,
                Decimal::new(90, 0),
            ))
            .expect("second portfolio snapshot should produce fail-closed rolling fact");

        assert_eq!(second.rolling_pnl, None);
    }

    #[test]
    fn loss_governor_feed_accumulates_position_adjustments() {
        let account_id = AccountId::from("POLYMARKET-001");
        let position_id = PositionId::from("P-001");
        let mut feed = BoltV3LossGovernorRuntimeFeed::new(account_id, 1_000);

        feed.record_portfolio_snapshot(&portfolio_snapshot(
            account_id,
            100,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::new(100, 0),
        ))
        .expect("initial portfolio snapshot should produce loss snapshot");

        feed.record_position_event(&position_changed_event_for_position(
            account_id,
            position_id,
            150,
            Decimal::ZERO,
            Decimal::new(-5, 0),
        ))
        .expect("position change should seed per-trade PnL");

        let adjusted = feed
            .record_position_event(&position_adjusted_event_for_position(
                account_id,
                position_id,
                160,
                Decimal::new(-1, 0),
            ))
            .expect("position adjustment should update per-trade PnL");

        assert_eq!(adjusted.per_trade_pnl, Some(Decimal::new(-6, 0)));
    }

    #[test]
    fn loss_governor_feed_refreshes_position_timestamp_on_adjustment_without_pnl_change() {
        let account_id = AccountId::from("POLYMARKET-001");
        let position_id = PositionId::from("P-001");
        let mut feed = BoltV3LossGovernorRuntimeFeed::new(account_id, 1_000);

        feed.record_portfolio_snapshot(&portfolio_snapshot(
            account_id,
            300,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::new(100, 0),
        ))
        .expect("portfolio snapshot should produce loss snapshot");

        feed.record_position_event(&position_changed_event_for_position(
            account_id,
            position_id,
            150,
            Decimal::ZERO,
            Decimal::new(-5, 0),
        ))
        .expect("position change should seed per-trade PnL");

        let adjusted = feed
            .record_position_event(&position_adjusted_event_for_position_with_pnl_change(
                account_id,
                position_id,
                250,
                None,
            ))
            .expect("position adjustment without PnL change should refresh per-trade timestamp");

        assert_eq!(adjusted.observed_at_ns, 250);
        assert_eq!(adjusted.per_trade_pnl, Some(Decimal::new(-5, 0)));
        assert_eq!(
            feed.latest_per_trade_pnl()
                .expect("per-trade PnL should remain populated")
                .observed_at_ns,
            250
        );
    }

    #[test]
    fn loss_governor_feed_removes_closed_positions_from_current_trade_context() {
        let account_id = AccountId::from("POLYMARKET-001");
        let position_id = PositionId::from("P-001");
        let mut feed = BoltV3LossGovernorRuntimeFeed::new(account_id, 1_000);

        feed.record_portfolio_snapshot(&portfolio_snapshot(
            account_id,
            100,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::new(100, 0),
        ))
        .expect("initial portfolio snapshot should produce loss snapshot");

        feed.record_position_event(&position_changed_event_for_position(
            account_id,
            position_id,
            150,
            Decimal::ZERO,
            Decimal::new(-20, 0),
        ))
        .expect("position change should seed per-trade PnL");

        let after_close = feed
            .record_position_event(&position_closed_event_for_position(
                account_id,
                position_id,
                160,
                Decimal::new(-20, 0),
                Decimal::ZERO,
            ))
            .expect("position close should publish current loss snapshot");

        assert_eq!(after_close.per_trade_pnl, None);

        let refreshed = feed
            .record_portfolio_snapshot(&portfolio_snapshot(
                account_id,
                200,
                Decimal::new(-20, 0),
                Decimal::ZERO,
                Decimal::new(80, 0),
            ))
            .expect("portfolio snapshot after position close should remain current");

        assert_eq!(refreshed.observed_at_ns, 200);
        assert_eq!(refreshed.per_trade_pnl, None);
    }

    #[test]
    fn loss_governor_feed_keeps_worst_position_pnl_across_positions() {
        let account_id = AccountId::from("POLYMARKET-001");
        let mut feed = BoltV3LossGovernorRuntimeFeed::new(account_id, 1_000);

        feed.record_portfolio_snapshot(&portfolio_snapshot(
            account_id,
            100,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::new(100, 0),
        ))
        .expect("initial portfolio snapshot should produce loss snapshot");

        feed.record_position_event(&position_changed_event_for_position(
            account_id,
            PositionId::from("P-001"),
            150,
            Decimal::ZERO,
            Decimal::new(-20, 0),
        ))
        .expect("first position event should update loss snapshot");

        let second_position = feed
            .record_position_event(&position_changed_event_for_position(
                account_id,
                PositionId::from("P-002"),
                160,
                Decimal::ZERO,
                Decimal::new(-1, 0),
            ))
            .expect("second position event should update loss snapshot");

        assert_eq!(second_position.per_trade_pnl, Some(Decimal::new(-20, 0)));
    }

    #[test]
    fn loss_governor_feed_fails_closed_on_mixed_currency_portfolio_facts() {
        let account_id = AccountId::from("POLYMARKET-001");
        let mut feed = BoltV3LossGovernorRuntimeFeed::new(account_id, 1_000);

        let snapshot = feed
            .record_portfolio_snapshot(&portfolio_snapshot_with_currencies(
                account_id,
                100,
                (
                    Decimal::new(-1, 0),
                    Currency::USD(),
                    Decimal::new(-2, 0),
                    Currency::BTC(),
                ),
                (Decimal::new(100, 0), Currency::USD()),
            ))
            .expect("matching mixed-currency portfolio snapshot should produce fail-closed loss snapshot");

        assert_eq!(snapshot.daily_pnl, None);
        assert_eq!(snapshot.rolling_pnl, None);
        assert_eq!(snapshot.current_equity, None);
        assert_eq!(snapshot.peak_equity, None);
    }

    #[test]
    fn loss_governor_feed_fails_closed_on_empty_portfolio_money_facts() {
        let account_id = AccountId::from("POLYMARKET-001");
        let mut feed = BoltV3LossGovernorRuntimeFeed::new(account_id, 1_000);

        let snapshot = feed
            .record_portfolio_snapshot(&portfolio_snapshot_with_money(
                account_id,
                100,
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Some(Currency::USD()),
            ))
            .expect("matching empty portfolio snapshot should produce fail-closed loss snapshot");

        assert_eq!(snapshot.daily_pnl, None);
        assert_eq!(snapshot.rolling_pnl, None);
        assert_eq!(snapshot.current_equity, None);
        assert_eq!(snapshot.peak_equity, None);
    }

    #[test]
    fn loss_governor_feed_preserves_peak_equity_across_invalid_portfolio_facts() {
        let account_id = AccountId::from("POLYMARKET-001");
        let mut feed = BoltV3LossGovernorRuntimeFeed::new(account_id, 1_000);

        feed.record_portfolio_snapshot(&portfolio_snapshot(
            account_id,
            100,
            Decimal::ZERO,
            Decimal::ZERO,
            Decimal::new(100, 0),
        ))
        .expect("initial portfolio snapshot should seed peak equity");

        feed.record_portfolio_snapshot(&portfolio_snapshot_with_currencies(
            account_id,
            150,
            (
                Decimal::new(-1, 0),
                Currency::USD(),
                Decimal::new(-2, 0),
                Currency::BTC(),
            ),
            (Decimal::new(90, 0), Currency::USD()),
        ))
        .expect("invalid portfolio snapshot should fail closed without clearing peak equity");

        let restored = feed
            .record_portfolio_snapshot(&portfolio_snapshot(
                account_id,
                200,
                Decimal::new(-5, 0),
                Decimal::ZERO,
                Decimal::new(90, 0),
            ))
            .expect("valid portfolio snapshot should restore portfolio facts");

        assert_eq!(restored.peak_equity, Some(Decimal::new(100, 0)));
        assert_eq!(restored.current_equity, Some(Decimal::new(90, 0)));
    }

    #[test]
    fn enabled_loss_governor_policy_conversion_requires_thresholds() {
        let mut loaded = fixture_loaded_config();
        let governor = loaded
            .root
            .risk
            .loss_governor
            .as_mut()
            .expect("fixture should configure loss governor");
        governor.max_daily_loss = None;

        let error = configured_loss_governor_policy(&loaded)
            .expect_err("enabled loss governor must reject missing thresholds");

        assert!(
            error
                .to_string()
                .contains("risk.loss_governor.max_daily_loss must be configured when enabled"),
            "unexpected loss-governor config error: {error}"
        );
    }

    #[test]
    fn enabled_loss_governor_policy_conversion_rejects_non_positive_runtime_values() {
        let mut zero_age = fixture_loaded_config();
        zero_age
            .root
            .risk
            .loss_governor
            .as_mut()
            .expect("fixture should configure loss governor")
            .max_snapshot_age_ns = 0;

        let zero_age_error = configured_loss_governor_policy(&zero_age)
            .expect_err("enabled loss governor must reject zero snapshot age");
        assert!(
            zero_age_error
                .to_string()
                .contains("risk.loss_governor.max_snapshot_age_ns must be a positive integer"),
            "unexpected loss-governor config error: {zero_age_error}"
        );

        let mut zero_daily = fixture_loaded_config();
        zero_daily
            .root
            .risk
            .loss_governor
            .as_mut()
            .expect("fixture should configure loss governor")
            .max_daily_loss = Some("0".to_string());

        let zero_daily_error = configured_loss_governor_policy(&zero_daily)
            .expect_err("enabled loss governor must reject zero daily threshold");
        assert!(
            zero_daily_error
                .to_string()
                .contains("risk.loss_governor.max_daily_loss must be a positive decimal string"),
            "unexpected loss-governor config error: {zero_daily_error}"
        );
    }

    #[test]
    fn enabled_loss_governor_policy_conversion_rejects_invalid_rolling_window() {
        let mut zero_window = fixture_loaded_config();
        zero_window
            .root
            .risk
            .loss_governor
            .as_mut()
            .expect("fixture should configure loss governor")
            .rolling_window_ns = 0;

        let zero_window_error = configured_loss_governor_policy(&zero_window)
            .expect_err("enabled loss governor must reject zero rolling window");
        assert!(
            zero_window_error
                .to_string()
                .contains("risk.loss_governor.rolling_window_ns must be a positive integer"),
            "unexpected loss-governor config error: {zero_window_error}"
        );
    }

    fn portfolio_snapshot(
        account_id: AccountId,
        ts_event: u64,
        realized_pnl: Decimal,
        unrealized_pnl: Decimal,
        total_equity: Decimal,
    ) -> PortfolioSnapshot {
        portfolio_snapshot_with_currencies(
            account_id,
            ts_event,
            (
                realized_pnl,
                Currency::USD(),
                unrealized_pnl,
                Currency::USD(),
            ),
            (total_equity, Currency::USD()),
        )
    }

    fn portfolio_snapshot_with_currencies(
        account_id: AccountId,
        ts_event: u64,
        pnl: (Decimal, Currency, Decimal, Currency),
        total_equity: (Decimal, Currency),
    ) -> PortfolioSnapshot {
        let (realized_pnl, realized_currency, unrealized_pnl, unrealized_currency) = pnl;
        let (total_equity, total_equity_currency) = total_equity;
        let unrealized_pnls =
            vec![Money::from_decimal(unrealized_pnl, unrealized_currency).unwrap()];
        let realized_pnls = vec![Money::from_decimal(realized_pnl, realized_currency).unwrap()];
        let total_equity = vec![Money::from_decimal(total_equity, total_equity_currency).unwrap()];
        portfolio_snapshot_with_money(
            account_id,
            ts_event,
            unrealized_pnls,
            realized_pnls,
            total_equity,
            Some(total_equity_currency),
        )
    }

    fn portfolio_snapshot_with_money(
        account_id: AccountId,
        ts_event: u64,
        unrealized_pnls: Vec<Money>,
        realized_pnls: Vec<Money>,
        total_equity: Vec<Money>,
        base_currency: Option<Currency>,
    ) -> PortfolioSnapshot {
        PortfolioSnapshot::new(
            account_id,
            AccountType::Betting,
            base_currency,
            vec![],
            vec![],
            unrealized_pnls,
            realized_pnls,
            total_equity,
            UUID4::default(),
            ts_event.into(),
            ts_event.into(),
        )
    }

    fn position_changed_event(
        account_id: AccountId,
        ts_event: u64,
        realized_pnl: Decimal,
        unrealized_pnl: Decimal,
    ) -> PositionEvent {
        position_changed_event_for_position(
            account_id,
            PositionId::from("P-001"),
            ts_event,
            realized_pnl,
            unrealized_pnl,
        )
    }

    fn position_changed_event_for_position(
        account_id: AccountId,
        position_id: PositionId,
        ts_event: u64,
        realized_pnl: Decimal,
        unrealized_pnl: Decimal,
    ) -> PositionEvent {
        PositionEvent::PositionChanged(PositionChanged {
            trader_id: TraderId::from("TESTER-001"),
            strategy_id: StrategyId::from("binary_oracle_edge_taker-001"),
            instrument_id: InstrumentId::from("BTCUSDT.BINANCE"),
            position_id,
            account_id,
            opening_order_id: ClientOrderId::from("O-001"),
            entry: OrderSide::Buy,
            side: PositionSide::Long,
            signed_qty: 1.0,
            quantity: Quantity::from("1"),
            peak_quantity: Quantity::from("1"),
            last_qty: Quantity::from("1"),
            last_px: Price::from("1"),
            currency: Currency::USD(),
            avg_px_open: 1.0,
            avg_px_close: None,
            realized_return: 0.0,
            realized_pnl: Some(Money::from_decimal(realized_pnl, Currency::USD()).unwrap()),
            unrealized_pnl: Money::from_decimal(unrealized_pnl, Currency::USD()).unwrap(),
            event_id: UUID4::default(),
            ts_opened: 1.into(),
            ts_event: ts_event.into(),
            ts_init: ts_event.into(),
        })
    }

    fn position_adjusted_event_for_position(
        account_id: AccountId,
        position_id: PositionId,
        ts_event: u64,
        pnl_change: Decimal,
    ) -> PositionEvent {
        position_adjusted_event_for_position_with_pnl_change(
            account_id,
            position_id,
            ts_event,
            Some(pnl_change),
        )
    }

    fn position_adjusted_event_for_position_with_pnl_change(
        account_id: AccountId,
        position_id: PositionId,
        ts_event: u64,
        pnl_change: Option<Decimal>,
    ) -> PositionEvent {
        PositionEvent::PositionAdjusted(PositionAdjusted {
            trader_id: TraderId::from("TESTER-001"),
            strategy_id: StrategyId::from("binary_oracle_edge_taker-001"),
            instrument_id: InstrumentId::from("BTCUSDT.BINANCE"),
            position_id,
            account_id,
            adjustment_type: PositionAdjustmentType::Funding,
            quantity_change: None,
            pnl_change: pnl_change.map(|pnl| Money::from_decimal(pnl, Currency::USD()).unwrap()),
            reason: None,
            event_id: UUID4::default(),
            ts_event: ts_event.into(),
            ts_init: ts_event.into(),
        })
    }

    fn position_closed_event_for_position(
        account_id: AccountId,
        position_id: PositionId,
        ts_event: u64,
        realized_pnl: Decimal,
        unrealized_pnl: Decimal,
    ) -> PositionEvent {
        PositionEvent::PositionClosed(PositionClosed {
            trader_id: TraderId::from("TESTER-001"),
            strategy_id: StrategyId::from("binary_oracle_edge_taker-001"),
            instrument_id: InstrumentId::from("BTCUSDT.BINANCE"),
            position_id,
            account_id,
            opening_order_id: ClientOrderId::from("O-001"),
            closing_order_id: Some(ClientOrderId::from("C-001")),
            entry: OrderSide::Buy,
            side: PositionSide::Long,
            signed_qty: 0.0,
            quantity: Quantity::zero(0),
            peak_quantity: Quantity::from("1"),
            last_qty: Quantity::from("1"),
            last_px: Price::from("1"),
            currency: Currency::USD(),
            avg_px_open: 1.0,
            avg_px_close: Some(1.0),
            realized_return: 0.0,
            realized_pnl: Some(Money::from_decimal(realized_pnl, Currency::USD()).unwrap()),
            unrealized_pnl: Money::from_decimal(unrealized_pnl, Currency::USD()).unwrap(),
            duration: nautilus_core::nanos::DurationNanos::from(1_u64),
            event_id: UUID4::default(),
            ts_opened: 1.into(),
            ts_closed: Some(ts_event.into()),
            ts_event: ts_event.into(),
            ts_init: ts_event.into(),
        })
    }

    #[test]
    fn no_submit_adapter_mapping_preserves_strategy_derived_market_filters() {
        use crate::{
            bolt_v3_providers::{
                binance::ResolvedBoltV3BinanceSecrets, polymarket::ResolvedBoltV3PolymarketSecrets,
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
                private_key: "fixture-poly-private-key".to_string(),
                api_key: "fixture-poly-api-key".to_string(),
                api_secret: "fixture-poly-api-secret".to_string(),
                passphrase: "fixture-poly-passphrase".to_string(),
            }),
        );
        clients.insert(
            "binance_reference".to_string(),
            Arc::new(ResolvedBoltV3BinanceSecrets {
                api_key: "fixture-binance-api-key".to_string(),
                api_secret: "fixture-binance-api-secret".to_string(),
            }),
        );
        let resolved = ResolvedBoltV3Secrets { clients };

        let adapters = no_submit_transport_adapter_configs(&loaded, &resolved)
            .expect("no-submit adapter mapping should retain market identity filters");
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
            "no-submit adapter mapping must keep strategy-derived provider filters"
        );
        assert_eq!(
            data.filters[0]
                .market_slugs()
                .expect("no-submit data config must keep configured target slug filters")
                .len(),
            2
        );
    }

    #[test]
    fn reference_quote_probe_does_not_satisfy_distinct_clients_with_one_quote() {
        let mut loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load before direct mutation");
        let strategy = loaded
            .strategies
            .first_mut()
            .expect("fixture should include one strategy");
        let primary = strategy
            .config
            .reference_data
            .get("primary")
            .expect("fixture should include primary reference data")
            .clone();
        strategy.config.reference_data.insert(
            "secondary".to_string(),
            ReferenceDataBlock {
                data_client_id: ClientId::from("polymarket_main"),
                instrument_id: primary.instrument_id,
            },
        );
        let handle = BoltV3NoSubmitReferenceQuoteProbeHandle::new(&loaded);
        let ambiguity = handle
            .ambiguity_error()
            .expect("probe setup should reject ambiguous reference quote sources");
        assert!(ambiguity.contains("QuoteTick does not carry data_client_id"));
        let quote = QuoteTick::new(
            primary.instrument_id,
            Price::from("100.00"),
            Price::from("100.01"),
            Quantity::from("1"),
            Quantity::from("1"),
            1_u64.into(),
            1_u64.into(),
        );

        handle.record_quote(&quote, 2);

        assert!(
            !handle.has_all_required_quotes(),
            "one source-unattributed QuoteTick must not satisfy distinct data clients"
        );
        assert_eq!(
            handle.evidence().quotes.len(),
            0,
            "probe must not label a source-unattributed QuoteTick with any ambiguous data client"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reference_quote_probe_wait_wakes_when_required_quote_records() {
        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let handle = BoltV3NoSubmitReferenceQuoteProbeHandle::new(&loaded);
        let required = handle
            .required
            .first()
            .expect("fixture should require reference quote evidence")
            .clone();
        let quote = QuoteTick::new(
            required.instrument_id,
            Price::from("100.00"),
            Price::from("100.01"),
            Quantity::from("1"),
            Quantity::from("1"),
            1_u64.into(),
            1_u64.into(),
        );
        let wait = handle.wait_for_all_required_quotes();
        tokio::pin!(wait);

        tokio::select! {
            () = &mut wait => panic!("wait should not complete before required quote evidence"),
            () = tokio::time::sleep(Duration::from_millis(5)) => {}
        }

        handle.record_quote(&quote, 2);
        tokio::time::timeout(Duration::from_millis(100), &mut wait)
            .await
            .expect("notify must wake required-quote wait");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reference_quote_probe_wait_accepts_quote_recorded_before_wait_starts() {
        let loaded = crate::bolt_v3_config::load_bolt_v3_config(std::path::Path::new(
            "tests/fixtures/bolt_v3/root.toml",
        ))
        .expect("fixture config should load");
        let handle = BoltV3NoSubmitReferenceQuoteProbeHandle::new(&loaded);
        let required = handle
            .required
            .first()
            .expect("fixture should require reference quote evidence")
            .clone();
        let quote = QuoteTick::new(
            required.instrument_id,
            Price::from("100.00"),
            Price::from("100.01"),
            Quantity::from("1"),
            Quantity::from("1"),
            1_u64.into(),
            1_u64.into(),
        );

        handle.record_quote(&quote, 2);
        tokio::time::timeout(
            Duration::from_millis(100),
            handle.wait_for_all_required_quotes(),
        )
        .await
        .expect("pre-observed quote must not be lost before wait starts");
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
    fn no_submit_timeout_sums_fail_closed_on_overflow() {
        let mut loaded = fixture_loaded_config();
        loaded.root.nautilus.timeout_connection_secs = u64::MAX;
        loaded.root.nautilus.timeout_reconciliation_secs = 1;
        let start_error = no_submit_start_timeout_secs(&loaded)
            .expect_err("no-submit start timeout overflow must fail closed");
        assert!(
            matches!(
                start_error,
                BoltV3LiveNodeError::NoSubmitStartTimeoutOverflow
            ),
            "expected start timeout overflow rejection, got {start_error:?}"
        );

        loaded.root.nautilus.timeout_disconnection_secs = u64::MAX;
        loaded.root.nautilus.delay_post_stop_secs = 1;
        let stop_error = no_submit_stop_timeout_secs(&loaded)
            .expect_err("no-submit stop timeout overflow must fail closed");
        assert!(
            matches!(stop_error, BoltV3LiveNodeError::NoSubmitStopTimeoutOverflow),
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
        assert_eq!(cfg.risk_engine.max_order_submit_rate, "100/00:00:01");
        assert_eq!(cfg.risk_engine.max_order_modify_rate, "100/00:00:01");
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
            .insert("ETHUSDT.BINANCE".to_string(), "12345.00".to_string());
        loaded
            .root
            .risk
            .nautilus
            .max_notional_per_order
            .insert("BTCUSDT.BINANCE".to_string(), "25000.50".to_string());
        let cfg = make_live_node_config(&loaded);

        assert_eq!(
            cfg.risk_engine
                .max_notional_per_order
                .get("ETHUSDT.BINANCE"),
            Some(&"12345.00".to_string())
        );
        assert_eq!(
            cfg.risk_engine
                .max_notional_per_order
                .get("BTCUSDT.BINANCE"),
            Some(&"25000.50".to_string())
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
