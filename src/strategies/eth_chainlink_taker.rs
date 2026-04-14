use std::{any::Any, cell::RefCell, collections::BTreeMap, rc::Rc};

use anyhow::{Context, Result};
use nautilus_common::{
    actor::{DataActor, registry::try_get_actor_unchecked},
    component::Component,
    msgbus::{self, ShareableMessageHandler},
};
use nautilus_model::identifiers::{InstrumentId, StrategyId};
#[cfg(not(test))]
use nautilus_model::enums::BookType;
use nautilus_system::trader::Trader;
use nautilus_trading::{StrategyConfig, StrategyCore, nautilus_strategy};
use serde::Deserialize;
use toml::Value;

use crate::{
    platform::{
        polymarket_catalog::polymarket_instrument_id,
        reference::ReferenceSnapshot,
        runtime::runtime_selection_topic,
        ruleset::{CandidateMarket, RuntimeSelectionSnapshot, SelectionState},
    },
    strategies::registry::{BoxedStrategy, StrategyBuildContext, StrategyBuilder},
    validate::ValidationError,
};

trait TomlValueExt {
    fn as_float_or_integer(&self) -> Option<f64>;
}

impl TomlValueExt for Value {
    fn as_float_or_integer(&self) -> Option<f64> {
        self.as_float()
            .or_else(|| self.as_integer().map(|value| value as f64))
    }
}

macro_rules! eth_chainlink_taker_config_fields {
    ($macro:ident) => {
        $macro! {
            strategy_id: String => as_str, "string", "a string", "missing_strategy_id";
            client_id: String => as_str, "string", "a string", "missing_client_id";
            warmup_tick_count: u64 => as_integer, "integer", "an integer", "missing_warmup_tick_count";
            reentry_cooldown_secs: u64 => as_integer, "integer", "an integer", "missing_reentry_cooldown_secs";
            max_position_usdc: f64 => as_float_or_integer, "float", "a float", "missing_max_position_usdc";
            book_impact_cap_bps: u64 => as_integer, "integer", "an integer", "missing_book_impact_cap_bps";
            risk_lambda: f64 => as_float_or_integer, "float", "a float", "missing_risk_lambda";
            worst_case_ev_min_bps: i64 => as_integer, "integer", "an integer", "missing_worst_case_ev_min_bps";
            exit_hysteresis_bps: i64 => as_integer, "integer", "an integer", "missing_exit_hysteresis_bps";
            forced_flat_stale_chainlink_ms: u64 => as_integer, "integer", "an integer", "missing_forced_flat_stale_chainlink_ms";
            forced_flat_thin_book_min_liquidity: f64 => as_float_or_integer, "float", "a float", "missing_forced_flat_thin_book_min_liquidity";
            lead_agreement_min_corr: f64 => as_float_or_integer, "float", "a float", "missing_lead_agreement_min_corr";
            lead_jitter_max_ms: u64 => as_integer, "integer", "an integer", "missing_lead_jitter_max_ms";
        }
    };
}

macro_rules! define_config_struct {
    ($( $field:ident : $ty:ty => $getter:ident, $expected:literal, $expected_with_article:literal, $missing_code:literal; )+) => {
        #[derive(Debug, Clone, PartialEq, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct EthChainlinkTakerConfig {
            $( $field: $ty, )+
        }
    };
}

macro_rules! match_config_field_names {
    ($( $field:ident : $ty:ty => $getter:ident, $expected:literal, $expected_with_article:literal, $missing_code:literal; )+) => {
        $( stringify!($field) )|+
    };
}

macro_rules! validate_config_fields_impl {
    ($( $field:ident : $ty:ty => $getter:ident, $expected:literal, $expected_with_article:literal, $missing_code:literal; )+) => {
        |table: &toml::map::Map<String, Value>, field_prefix: &str, errors: &mut Vec<ValidationError>| {
            $(
                let field = format!("{field_prefix}.{}", stringify!($field));
                match table.get(stringify!($field)) {
                    None => EthChainlinkTakerBuilder::push_missing(errors, field, $missing_code, $expected),
                    Some(value) if value.$getter().is_none() => {
                        EthChainlinkTakerBuilder::push_wrong_type(errors, field, $expected_with_article, value);
                    }
                    Some(_) => {}
                }
            )+
        }
    };
}

eth_chainlink_taker_config_fields!(define_config_struct);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum SelectionPhase {
    Active,
    Freeze,
    #[default]
    Idle,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct ActiveMarketState {
    phase: SelectionPhase,
    market_id: Option<String>,
    instrument_id: Option<InstrumentId>,
    outcome_fees: OutcomeFeeState,
    interval_start_ms: Option<u64>,
    interval_open: Option<f64>,
    last_reference_ts_ms: Option<u64>,
    warmup_count: u64,
    warmup_target: u64,
    forced_flat: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OutcomeBookSubscriptions {
    up_instrument_id: Option<InstrumentId>,
    down_instrument_id: Option<InstrumentId>,
}

impl OutcomeBookSubscriptions {
    fn from_market(market: &CandidateMarket) -> Self {
        Self {
            up_instrument_id: Some(polymarket_instrument_id(&market.condition_id, &market.up_token_id)),
            down_instrument_id: Some(polymarket_instrument_id(
                &market.condition_id,
                &market.down_token_id,
            )),
        }
    }

    fn is_same_market(&self, other: &Self) -> bool {
        self.up_instrument_id == other.up_instrument_id
            && self.down_instrument_id == other.down_instrument_id
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OutcomeFeeState {
    up_token_id: Option<String>,
    down_token_id: Option<String>,
    up_ready: bool,
    down_ready: bool,
}

impl OutcomeFeeState {
    fn from_market(market: &CandidateMarket) -> Self {
        Self {
            up_token_id: Some(market.up_token_id.clone()),
            down_token_id: Some(market.down_token_id.clone()),
            ..Self::default()
        }
    }

    fn token_ids(&self) -> Vec<String> {
        [self.up_token_id.clone(), self.down_token_id.clone()]
            .into_iter()
            .flatten()
            .collect()
    }

}

impl ActiveMarketState {
    fn from_snapshot(snapshot: &RuntimeSelectionSnapshot, warmup_target: u64) -> Self {
        match &snapshot.decision.state {
            SelectionState::Active { market } => {
                Self::from_market(market, warmup_target, SelectionPhase::Active, false)
            }
            SelectionState::Freeze { market, .. } => {
                Self::from_market(market, warmup_target, SelectionPhase::Freeze, true)
            }
            SelectionState::Idle { .. } => Self {
                phase: SelectionPhase::Idle,
                forced_flat: true,
                ..Self::default()
            },
        }
    }

    fn from_market(
        market: &CandidateMarket,
        warmup_target: u64,
        phase: SelectionPhase,
        forced_flat: bool,
    ) -> Self {
        Self {
            phase,
            market_id: Some(market.market_id.clone()),
            instrument_id: Some(InstrumentId::from(market.instrument_id.as_str())),
            outcome_fees: OutcomeFeeState::from_market(market),
            interval_start_ms: Some(market.start_ts_ms),
            warmup_target,
            forced_flat,
            ..Self::default()
        }
    }

    fn same_boundary(&self, other: &Self) -> bool {
        self.phase == other.phase
            && self.market_id == other.market_id
            && self.instrument_id == other.instrument_id
            && self.interval_start_ms == other.interval_start_ms
    }

    fn warmup_complete(&self) -> bool {
        self.warmup_target > 0 && self.warmup_count >= self.warmup_target
    }

    fn observe_reference_snapshot(&mut self, snapshot: &ReferenceSnapshot) {
        if self.phase != SelectionPhase::Active || self.forced_flat {
            return;
        }
        let Some(interval_start_ms) = self.interval_start_ms else {
            return;
        };
        let Some(fair_value) = snapshot.fair_value else {
            return;
        };
        if snapshot.ts_ms < interval_start_ms {
            return;
        }
        if self
            .last_reference_ts_ms
            .is_some_and(|last_ts_ms| snapshot.ts_ms <= last_ts_ms)
        {
            return;
        }

        self.last_reference_ts_ms = Some(snapshot.ts_ms);
        if self.interval_open.is_none() {
            self.interval_open = Some(fair_value);
        }
        self.warmup_count += 1;
    }
}

pub struct EthChainlinkTaker {
    core: StrategyCore,
    config: EthChainlinkTakerConfig,
    context: StrategyBuildContext,
    active: ActiveMarketState,
    book_subscriptions: OutcomeBookSubscriptions,
    cooldowns: BTreeMap<String, u64>,
    recovery: bool,
    selection_handler: Option<ShareableMessageHandler>,
    reference_handler: Option<ShareableMessageHandler>,
    #[cfg(test)]
    book_subscription_events: Vec<BookSubscriptionEvent>,
}

impl EthChainlinkTaker {
    fn new(config: EthChainlinkTakerConfig, context: StrategyBuildContext) -> Self {
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(StrategyId::from(config.strategy_id.as_str())),
                    ..Default::default()
                }),
            config,
            context,
            active: ActiveMarketState::default(),
            book_subscriptions: OutcomeBookSubscriptions::default(),
            cooldowns: BTreeMap::new(),
            recovery: false,
            selection_handler: None,
            reference_handler: None,
            #[cfg(test)]
            book_subscription_events: Vec::new(),
        }
    }

    fn apply_selection_snapshot(&mut self, snapshot: RuntimeSelectionSnapshot) {
        let previous_phase = self.active.phase;
        let previous_fee_tokens = self.active.outcome_fees.token_ids();
        let next_books = selection_book_subscriptions(&snapshot);
        let should_replace_books = should_replace_book_subscriptions(&self.book_subscriptions, &next_books);
        if should_replace_books {
            self.replace_book_subscriptions(next_books.clone());
        }
        apply_selection_snapshot_to_active(
            &mut self.active,
            &snapshot,
            self.config.warmup_tick_count,
        );
        let reactivated_into_active =
            previous_phase != SelectionPhase::Active && self.active.phase == SelectionPhase::Active;
        let next_fee_tokens = self.active.outcome_fees.token_ids();
        if previous_fee_tokens != next_fee_tokens
            || (reactivated_into_active && !next_fee_tokens.is_empty())
        {
            self.trigger_fee_warm_for_market();
        }
    }

    fn observe_reference_snapshot(&mut self, snapshot: &ReferenceSnapshot) {
        self.active.observe_reference_snapshot(snapshot);
    }

    fn refresh_fee_readiness(&mut self) {
        refresh_fee_readiness_for_active(
            &mut self.active,
            self.context.fee_provider.as_ref(),
        );
    }

    fn trigger_fee_warm_for_market(&self) {
        let token_ids = self.active.outcome_fees.token_ids();
        if token_ids.is_empty() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for token_id in token_ids {
            let fee_provider = self.context.fee_provider.clone();
            handle.spawn(async move {
                let _ = fee_provider.warm(&token_id).await;
            });
        }
    }

    fn register_shell_subscriptions(&mut self) {
        let actor_id = self.actor_id().inner();
        let selection_topic = runtime_selection_topic(&StrategyId::from(self.config.strategy_id.as_str()));
        let selection_handler = ShareableMessageHandler::from_any(move |message: &dyn Any| {
            let Some(snapshot) = message.downcast_ref::<RuntimeSelectionSnapshot>() else {
                return;
            };
            if let Some(mut actor) = try_get_actor_unchecked::<EthChainlinkTaker>(&actor_id) {
                actor.apply_selection_snapshot(snapshot.clone());
            }
        });
        msgbus::subscribe_any(selection_topic.into(), selection_handler.clone(), None);
        self.selection_handler = Some(selection_handler);

        let actor_id = self.actor_id().inner();
        let reference_topic = self.context.reference_publish_topic.clone();
        let reference_handler = ShareableMessageHandler::from_any(move |message: &dyn Any| {
            let Some(snapshot) = message.downcast_ref::<ReferenceSnapshot>() else {
                return;
            };
            if let Some(mut actor) = try_get_actor_unchecked::<EthChainlinkTaker>(&actor_id) {
                actor.observe_reference_snapshot(snapshot);
            }
        });
        msgbus::subscribe_any(reference_topic.into(), reference_handler.clone(), None);
        self.reference_handler = Some(reference_handler);
    }

    fn deregister_shell_subscriptions(&mut self) {
        let selection_topic = runtime_selection_topic(&StrategyId::from(self.config.strategy_id.as_str()));
        if let Some(handler) = self.selection_handler.take() {
            msgbus::unsubscribe_any(selection_topic.into(), &handler);
        }
        if let Some(handler) = self.reference_handler.take() {
            msgbus::unsubscribe_any(self.context.reference_publish_topic.clone().into(), &handler);
        }
        self.replace_book_subscriptions(OutcomeBookSubscriptions::default());
    }

    fn replace_book_subscriptions(&mut self, next: OutcomeBookSubscriptions) {
        let current = self.book_subscriptions.clone();
        unsubscribe_missing_books(self, &current, &next);
        subscribe_new_books(self, &current, &next);
        self.book_subscriptions = next;
    }
}

impl std::fmt::Debug for EthChainlinkTaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EthChainlinkTaker")
            .field("config", &self.config)
            .finish()
    }
}

impl DataActor for EthChainlinkTaker {
    fn on_start(&mut self) -> Result<()> {
        self.register_shell_subscriptions();
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        self.deregister_shell_subscriptions();
        Ok(())
    }
}

nautilus_strategy!(EthChainlinkTaker);

#[derive(Debug)]
pub struct EthChainlinkTakerBuilder;

impl EthChainlinkTakerBuilder {
    fn parse_config(raw: &Value) -> Result<EthChainlinkTakerConfig> {
        raw.clone()
            .try_into()
            .context("eth_chainlink_taker builder requires a valid config table")
    }

    fn push_missing(
        errors: &mut Vec<ValidationError>,
        field: String,
        code: &'static str,
        expected: &'static str,
    ) {
        errors.push(ValidationError {
            field,
            code,
            message: format!("is missing required {expected} field"),
        });
    }

    fn push_wrong_type(
        errors: &mut Vec<ValidationError>,
        field: String,
        expected_with_article: &'static str,
        value: &Value,
    ) {
        errors.push(ValidationError {
            field,
            code: "wrong_type",
            message: format!(
                "must be {expected_with_article}, got {} value",
                value.type_str()
            ),
        });
    }

    fn push_unknown_field(errors: &mut Vec<ValidationError>, field: String, key: &str) {
        errors.push(ValidationError {
            field,
            code: "unknown_field",
            message: format!("unknown field `{key}`"),
        });
    }

    fn validate_table(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        for key in table.keys() {
            if !matches!(
                key.as_str(),
                eth_chainlink_taker_config_fields!(match_config_field_names)
            ) {
                Self::push_unknown_field(errors, format!("{field_prefix}.{key}"), key);
            }
        }

        eth_chainlink_taker_config_fields!(validate_config_fields_impl)(
            table,
            field_prefix,
            errors,
        );
    }
}

impl StrategyBuilder for EthChainlinkTakerBuilder {
    fn kind() -> &'static str {
        "eth_chainlink_taker"
    }

    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
        let Some(table) = raw.as_table() else {
            Self::push_wrong_type(errors, field_prefix.to_string(), "a table", raw);
            return;
        };

        Self::validate_table(table, field_prefix, errors);
    }

    fn build(raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        Ok(Box::new(EthChainlinkTaker::new(
            Self::parse_config(raw)?,
            context.clone(),
        )))
    }

    fn register(
        raw: &Value,
        context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId> {
        let strategy = EthChainlinkTaker::new(Self::parse_config(raw)?, context.clone());
        let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());
        trader.borrow_mut().add_strategy(strategy)?;
        Ok(strategy_id)
    }
}

fn apply_selection_snapshot_to_active(
    active: &mut ActiveMarketState,
    snapshot: &RuntimeSelectionSnapshot,
    warmup_target: u64,
) {
    let next = ActiveMarketState::from_snapshot(snapshot, warmup_target);
    if active.same_boundary(&next) {
        return;
    }
    if same_market_transition(active, &next) {
        active.phase = next.phase;
        active.forced_flat = next.forced_flat;
        return;
    }
    *active = next;
}

fn same_market_transition(current: &ActiveMarketState, next: &ActiveMarketState) -> bool {
    current.market_id.is_some()
        && current.market_id == next.market_id
        && current.instrument_id == next.instrument_id
        && current.interval_start_ms == next.interval_start_ms
}

fn selection_book_subscriptions(snapshot: &RuntimeSelectionSnapshot) -> OutcomeBookSubscriptions {
    match &snapshot.decision.state {
        SelectionState::Active { market } | SelectionState::Freeze { market, .. } => {
            OutcomeBookSubscriptions::from_market(market)
        }
        SelectionState::Idle { .. } => OutcomeBookSubscriptions::default(),
    }
}

fn should_replace_book_subscriptions(
    current: &OutcomeBookSubscriptions,
    next: &OutcomeBookSubscriptions,
) -> bool {
    !current.is_same_market(next)
}

fn unsubscribe_missing_books(
    strategy: &mut EthChainlinkTaker,
    current: &OutcomeBookSubscriptions,
    next: &OutcomeBookSubscriptions,
) {
    if let Some(instrument_id) = current.up_instrument_id
        && next.up_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.unsubscribe_book_deltas(instrument_id, None, None);
        strategy.record_book_subscription_event(BookSubscriptionEvent::unsubscribe(instrument_id));
    }
    if let Some(instrument_id) = current.down_instrument_id
        && next.down_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.unsubscribe_book_deltas(instrument_id, None, None);
        strategy.record_book_subscription_event(BookSubscriptionEvent::unsubscribe(instrument_id));
    }
}

fn subscribe_new_books(
    strategy: &mut EthChainlinkTaker,
    current: &OutcomeBookSubscriptions,
    next: &OutcomeBookSubscriptions,
) {
    if let Some(instrument_id) = next.up_instrument_id
        && current.up_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.subscribe_book_deltas(instrument_id, BookType::L2_MBP, None, None, false, None);
        strategy.record_book_subscription_event(BookSubscriptionEvent::subscribe(instrument_id));
    }
    if let Some(instrument_id) = next.down_instrument_id
        && current.down_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.subscribe_book_deltas(instrument_id, BookType::L2_MBP, None, None, false, None);
        strategy.record_book_subscription_event(BookSubscriptionEvent::subscribe(instrument_id));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BookSubscriptionEvent {
    action: &'static str,
    instrument_id: InstrumentId,
}

impl BookSubscriptionEvent {
    fn subscribe(instrument_id: InstrumentId) -> Self {
        Self {
            action: "subscribe",
            instrument_id,
        }
    }

    fn unsubscribe(instrument_id: InstrumentId) -> Self {
        Self {
            action: "unsubscribe",
            instrument_id,
        }
    }
}

impl EthChainlinkTaker {
    fn record_book_subscription_event(&mut self, event: BookSubscriptionEvent) {
        #[cfg(test)]
        self.book_subscription_events.push(event);
        #[cfg(not(test))]
        let _ = event;
    }
}

fn refresh_fee_readiness_for_active(
    active: &mut ActiveMarketState,
    fee_provider: &dyn crate::clients::polymarket::FeeProvider,
) {
    active.outcome_fees.up_ready = active
        .outcome_fees
        .up_token_id
        .as_deref()
        .and_then(|token_id| fee_provider.fee_bps(token_id))
        .is_some();
    active.outcome_fees.down_ready = active
        .outcome_fees
        .down_token_id
        .as_deref()
        .and_then(|token_id| fee_provider.fee_bps(token_id))
        .is_some();
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use anyhow::Result;
    use futures_util::future::{BoxFuture, FutureExt};
    use nautilus_common::{
        actor::registry::{get_actor_registry, get_actor_unchecked, register_actor},
        msgbus,
    };
    use rust_decimal::Decimal;

    use super::*;
    use crate::{
        platform::{
            reference::ReferenceSnapshot,
            resolution_basis::parse_ruleset_resolution_basis,
            ruleset::{CandidateMarket, RuntimeSelectionSnapshot, SelectionDecision, SelectionState},
        },
        strategies::{production_strategy_registry, registry::StrategyBuilder},
    };

    fn find_error<'a>(
        errors: &'a [ValidationError],
        field: &str,
        code: &'static str,
    ) -> &'a ValidationError {
        errors
            .iter()
            .find(|e| e.field == field && e.code == code)
            .unwrap_or_else(|| panic!("missing error {field} / {code}: {errors:?}"))
    }

    fn valid_raw_config() -> Value {
        toml::toml! {
            strategy_id = "ETHCHAINLINKTAKER-001"
            client_id = "POLYMARKET"
            warmup_tick_count = 20
            reentry_cooldown_secs = 30
            max_position_usdc = 1000.0
            book_impact_cap_bps = 15
            risk_lambda = 0.5
            worst_case_ev_min_bps = -20
            exit_hysteresis_bps = 5
            forced_flat_stale_chainlink_ms = 1500
            forced_flat_thin_book_min_liquidity = 100.0
            lead_agreement_min_corr = 0.8
            lead_jitter_max_ms = 250
        }
        .into()
    }

    #[derive(Debug, Default)]
    struct RecordingFeeProvider {
        fees: Mutex<HashMap<String, Decimal>>,
        warm_calls: Mutex<Vec<String>>,
    }

    impl RecordingFeeProvider {
        fn cold() -> Arc<Self> {
            Arc::new(Self::default())
        }

        fn set_fee(&self, token_id: &str, fee_bps: Decimal) {
            self.fees
                .lock()
                .expect("recording fee provider mutex poisoned")
                .insert(token_id.to_string(), fee_bps);
        }

        fn warm_calls(&self) -> Vec<String> {
            self.warm_calls
                .lock()
                .expect("recording fee provider mutex poisoned")
                .clone()
        }
    }

    impl crate::clients::polymarket::FeeProvider for RecordingFeeProvider {
        fn fee_bps(&self, token_id: &str) -> Option<Decimal> {
            self.fees
                .lock()
                .expect("recording fee provider mutex poisoned")
                .get(token_id)
                .copied()
        }

        fn warm(&self, token_id: &str) -> BoxFuture<'_, Result<()>> {
            self.warm_calls
                .lock()
                .expect("recording fee provider mutex poisoned")
                .push(token_id.to_string());
            async { Ok(()) }.boxed()
        }
    }

    fn test_strategy() -> EthChainlinkTaker {
        test_strategy_with_fee_provider(RecordingFeeProvider::cold())
    }

    fn test_strategy_with_fee_provider(
        fee_provider: Arc<dyn crate::clients::polymarket::FeeProvider>,
    ) -> EthChainlinkTaker {
        EthChainlinkTaker::new(
            EthChainlinkTakerConfig {
                strategy_id: "ETHCHAINLINKTAKER-001".to_string(),
                client_id: "POLYMARKET".to_string(),
                warmup_tick_count: 20,
                reentry_cooldown_secs: 30,
                max_position_usdc: 1000.0,
                book_impact_cap_bps: 15,
                risk_lambda: 0.5,
                worst_case_ev_min_bps: -20,
                exit_hysteresis_bps: 5,
                forced_flat_stale_chainlink_ms: 1500,
                forced_flat_thin_book_min_liquidity: 100.0,
                lead_agreement_min_corr: 0.8,
                lead_jitter_max_ms: 250,
            },
            StrategyBuildContext {
                fee_provider,
                reference_publish_topic: "platform.reference.test.chainlink".to_string(),
            },
        )
    }

    fn active_snapshot(market_id: &str) -> RuntimeSelectionSnapshot {
        active_snapshot_with_start(market_id, 0)
    }

    fn active_snapshot_with_start(
        market_id: &str,
        interval_start_ms: u64,
    ) -> RuntimeSelectionSnapshot {
        selection_snapshot(
            interval_start_ms,
            SelectionState::Active {
                market: candidate_market(market_id, interval_start_ms),
            },
        )
    }

    fn freeze_snapshot_with_start(
        market_id: &str,
        interval_start_ms: u64,
    ) -> RuntimeSelectionSnapshot {
        selection_snapshot(
            interval_start_ms,
            SelectionState::Freeze {
                market: candidate_market(market_id, interval_start_ms),
                reason: "freeze window".to_string(),
            },
        )
    }

    fn selection_snapshot(interval_start_ms: u64, state: SelectionState) -> RuntimeSelectionSnapshot {
        RuntimeSelectionSnapshot {
            ruleset_id: "ETHCHAINLINKTAKER".to_string(),
            decision: SelectionDecision {
                ruleset_id: "ETHCHAINLINKTAKER".to_string(),
                state,
            },
            eligible_candidates: Vec::new(),
            published_at_ms: interval_start_ms,
        }
    }

    fn candidate_market(market_id: &str, interval_start_ms: u64) -> CandidateMarket {
        let condition_id = format!("condition-{market_id}");
        let up_token_id = format!("{market_id}-UP");
        let down_token_id = format!("{market_id}-DOWN");
        CandidateMarket {
            market_id: market_id.to_string(),
            instrument_id: polymarket_instrument_id(&condition_id, &up_token_id).to_string(),
            condition_id,
            up_token_id,
            down_token_id,
            start_ts_ms: interval_start_ms,
            declared_resolution_basis: parse_ruleset_resolution_basis("chainlink_ethusd")
                .expect("test resolution basis should parse"),
            accepting_orders: true,
            liquidity_num: 1000.0,
            seconds_to_end: 600,
        }
    }

    fn reference_tick(timestamp_ms: u64, price: f64) -> ReferenceSnapshot {
        ReferenceSnapshot {
            ts_ms: timestamp_ms,
            topic: "platform.reference.test.chainlink".to_string(),
            fair_value: Some(price),
            confidence: 1.0,
            venues: Vec::new(),
        }
    }

    #[test]
    fn production_registry_registers_eth_chainlink_taker_kind() {
        let registry = production_strategy_registry().expect("registry should build");
        assert!(registry.get("eth_chainlink_taker").is_some());
    }

    #[test]
    fn builder_requires_strategy_id_and_client_id() {
        let raw = toml::toml! {
            warmup_tick_count = 20
            reentry_cooldown_secs = 30
            max_position_usdc = 1000.0
            book_impact_cap_bps = 15
            risk_lambda = 0.5
            worst_case_ev_min_bps = -20
            exit_hysteresis_bps = 5
            forced_flat_stale_chainlink_ms = 1500
            forced_flat_thin_book_min_liquidity = 100.0
            lead_agreement_min_corr = 0.8
            lead_jitter_max_ms = 250
        }
        .into();
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.strategy_id")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.client_id")
        );
    }

    #[test]
    fn builder_rejects_unknown_fields() {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("valid config must be a table")
            .insert("stray_flag".to_string(), Value::Boolean(true));
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        let error = find_error(&errors, "strategies[0].config.stray_flag", "unknown_field");
        assert!(error.message.contains("unknown field `stray_flag`"));
    }

    #[test]
    fn builder_rejects_non_table_config() {
        let raw = Value::String("not-a-table".to_string());
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        let error = find_error(&errors, "strategies[0].config", "wrong_type");
        assert_eq!(error.message, "must be a table, got string value");
        assert!(!errors.iter().any(|e| {
            e.field == "strategies[0].config.strategy_id" && e.code == "missing_required_string"
        }));
    }

    #[test]
    fn builder_rejects_wrong_type_config_at_the_field() {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("valid config must be a table")
            .insert(
                "warmup_tick_count".to_string(),
                Value::String("20".to_string()),
            );
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        let error = find_error(
            &errors,
            "strategies[0].config.warmup_tick_count",
            "wrong_type",
        );
        assert_eq!(error.message, "must be an integer, got string value");
        assert!(!errors.iter().any(|e| {
            e.field == "strategies[0].config.warmup_tick_count"
                && e.code == "missing_required_integer"
        }));
    }

    #[test]
    fn builder_accepts_integer_literals_for_f64_fields() {
        let raw = toml::toml! {
            strategy_id = "ETHCHAINLINKTAKER-001"
            client_id = "POLYMARKET"
            warmup_tick_count = 20
            reentry_cooldown_secs = 30
            max_position_usdc = 1000
            book_impact_cap_bps = 15
            risk_lambda = 1
            worst_case_ev_min_bps = -20
            exit_hysteresis_bps = 5
            forced_flat_stale_chainlink_ms = 1500
            forced_flat_thin_book_min_liquidity = 100
            lead_agreement_min_corr = 1
            lead_jitter_max_ms = 250
        }
        .into();
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            !errors
                .iter()
                .any(|e| e.code == "wrong_type" && e.field.starts_with("strategies[0].config")),
            "expected integer literals for f64 fields to validate, got: {errors:?}"
        );
    }

    #[test]
    fn switch_resets_only_active_market_state() {
        let mut strategy = test_strategy();
        strategy.cooldowns.insert("A".to_string(), 123);
        strategy.recovery = true;
        {
            let active = &mut strategy.active;
            active.interval_open = Some(3_000.0);
            active.warmup_count = 7;
        }

        strategy.apply_selection_snapshot(active_snapshot("B"));

        assert_eq!(strategy.cooldowns.get("A"), Some(&123));
        assert!(strategy.recovery);
        let active = &strategy.active;
        assert_eq!(active.market_id.as_deref(), Some("B"));
        assert!(active.interval_open.is_none());
        assert_eq!(active.warmup_count, 0);
        assert!(!active.outcome_fees.up_ready);
        assert!(!active.outcome_fees.down_ready);
    }

    #[test]
    fn interval_open_captures_first_chainlink_tick_at_or_after_market_start() {
        let mut strategy = test_strategy();
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

        strategy.observe_reference_snapshot(&reference_tick(900, 3_100.0));
        assert!(strategy.active.interval_open.is_none());

        strategy.observe_reference_snapshot(&reference_tick(1_000, 3_101.0));
        assert_eq!(strategy.active.interval_open, Some(3_101.0));
    }

    #[test]
    fn fees_ready_requires_both_outcome_tokens_before_refresh_can_succeed() {
        let fee_provider = RecordingFeeProvider::cold();
        let mut strategy = test_strategy_with_fee_provider(fee_provider.clone());
        strategy.apply_selection_snapshot(active_snapshot("MKT-1"));

        assert!(!strategy.active.outcome_fees.up_ready);
        assert!(!strategy.active.outcome_fees.down_ready);

        fee_provider.set_fee("MKT-1-UP", Decimal::new(175, 2));
        strategy.refresh_fee_readiness();
        assert!(strategy.active.outcome_fees.up_ready);
        assert!(!strategy.active.outcome_fees.down_ready);

        fee_provider.set_fee("MKT-1-DOWN", Decimal::new(180, 2));
        strategy.refresh_fee_readiness();

        assert!(strategy.active.outcome_fees.up_ready);
        assert!(strategy.active.outcome_fees.down_ready);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn market_activation_and_switch_warm_both_outcome_fee_tokens() {
        let fee_provider = RecordingFeeProvider::cold();
        let mut strategy = test_strategy_with_fee_provider(fee_provider.clone());

        strategy.apply_selection_snapshot(active_snapshot("MKT-1"));
        tokio::task::yield_now().await;
        strategy.apply_selection_snapshot(active_snapshot("MKT-2"));
        tokio::task::yield_now().await;

        assert_eq!(
            fee_provider.warm_calls(),
            vec![
                "MKT-1-UP".to_string(),
                "MKT-1-DOWN".to_string(),
                "MKT-2-UP".to_string(),
                "MKT-2-DOWN".to_string(),
            ]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn same_market_freeze_to_active_reactivation_warms_fees_again() {
        let fee_provider = RecordingFeeProvider::cold();
        let mut strategy = test_strategy_with_fee_provider(fee_provider.clone());

        strategy.apply_selection_snapshot(freeze_snapshot_with_start("MKT-1", 0));
        tokio::task::yield_now().await;
        strategy.apply_selection_snapshot(active_snapshot("MKT-1"));
        tokio::task::yield_now().await;

        assert_eq!(
            fee_provider.warm_calls(),
            vec![
                "MKT-1-UP".to_string(),
                "MKT-1-DOWN".to_string(),
                "MKT-1-UP".to_string(),
                "MKT-1-DOWN".to_string(),
            ]
        );
    }

    #[test]
    fn fee_readiness_stays_false_until_both_outcome_fees_are_available() {
        let fee_provider = RecordingFeeProvider::cold();
        let mut strategy = test_strategy_with_fee_provider(fee_provider.clone());
        strategy.apply_selection_snapshot(active_snapshot("MKT-1"));

        assert!(!strategy.active.outcome_fees.up_ready);
        assert!(!strategy.active.outcome_fees.down_ready);

        fee_provider.set_fee("MKT-1-UP", Decimal::new(175, 2));
        strategy.refresh_fee_readiness();
        assert!(strategy.active.outcome_fees.up_ready);
        assert!(!strategy.active.outcome_fees.down_ready);

        fee_provider.set_fee("MKT-1-DOWN", Decimal::new(180, 2));
        strategy.refresh_fee_readiness();
        assert!(strategy.active.outcome_fees.up_ready);
        assert!(strategy.active.outcome_fees.down_ready);
    }

    #[test]
    fn same_market_active_to_freeze_updates_forced_flat_without_resetting_shell_state() {
        let mut strategy = test_strategy();
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
        {
            let active = &mut strategy.active;
            active.interval_open = Some(3_100.0);
            active.warmup_count = 2;
            active.outcome_fees.up_ready = true;
            active.outcome_fees.down_ready = true;
            active.forced_flat = false;
        }

        strategy.apply_selection_snapshot(freeze_snapshot_with_start("MKT-1", 1_000));

        let active = &strategy.active;
        assert_eq!(active.market_id.as_deref(), Some("MKT-1"));
        assert!(active.forced_flat);
        assert_eq!(active.interval_open, Some(3_100.0));
        assert_eq!(active.warmup_count, 2);
        assert!(active.outcome_fees.up_ready);
        assert!(active.outcome_fees.down_ready);
        assert_eq!(active.outcome_fees.up_token_id.as_deref(), Some("MKT-1-UP"));
        assert_eq!(active.outcome_fees.down_token_id.as_deref(), Some("MKT-1-DOWN"));
    }

    #[test]
    fn switch_resets_fee_readiness_fail_closed_even_if_provider_has_cached_fee() {
        let fee_provider = RecordingFeeProvider::cold();
        fee_provider.set_fee("MKT-1-UP", Decimal::new(175, 2));
        fee_provider.set_fee("MKT-1-DOWN", Decimal::new(180, 2));
        let mut strategy = test_strategy_with_fee_provider(fee_provider);
        {
            let active = &mut strategy.active;
            active.outcome_fees.up_ready = true;
            active.outcome_fees.down_ready = true;
        }

        strategy.apply_selection_snapshot(active_snapshot("MKT-2"));

        assert!(!strategy.active.outcome_fees.up_ready);
        assert!(!strategy.active.outcome_fees.down_ready);
    }

    #[test]
    fn market_switch_replaces_both_outcome_book_subscriptions() {
        let mut strategy = test_strategy();

        strategy.apply_selection_snapshot(active_snapshot("A"));
        strategy.book_subscription_events.clear();

        strategy.apply_selection_snapshot(active_snapshot("B"));

        assert_eq!(
            strategy.book_subscription_events,
            vec![
                BookSubscriptionEvent::unsubscribe(InstrumentId::from("condition-A-A-UP.POLYMARKET")),
                BookSubscriptionEvent::unsubscribe(InstrumentId::from("condition-A-A-DOWN.POLYMARKET")),
                BookSubscriptionEvent::subscribe(InstrumentId::from("condition-B-B-UP.POLYMARKET")),
                BookSubscriptionEvent::subscribe(InstrumentId::from("condition-B-B-DOWN.POLYMARKET")),
            ]
        );
    }

    #[test]
    fn runtime_selection_msgbus_callback_drives_full_strategy_selection_path() {
        let strategy = test_strategy();
        let selection_topic = runtime_selection_topic(&StrategyId::from("ETHCHAINLINKTAKER-001"));
        let actor_rc = register_actor(strategy);

        unsafe {
            DataActor::on_start(&mut *actor_rc.get())
                .expect("registered actor should subscribe cleanly");
        }

        msgbus::publish_any(selection_topic.into(), &active_snapshot("A"));
        unsafe {
            (&mut *actor_rc.get()).book_subscription_events.clear();
        }

        msgbus::publish_any(runtime_selection_topic(&StrategyId::from("ETHCHAINLINKTAKER-001")).into(), &active_snapshot("B"));

        let actor_id = unsafe { (&*actor_rc.get()).actor_id().inner() };
        let actor_ref = get_actor_unchecked::<EthChainlinkTaker>(&actor_id);
        assert_eq!(actor_ref.active.market_id.as_deref(), Some("B"));
        assert_eq!(
            actor_ref.book_subscription_events,
            vec![
                BookSubscriptionEvent::unsubscribe(InstrumentId::from("condition-A-A-UP.POLYMARKET")),
                BookSubscriptionEvent::unsubscribe(InstrumentId::from("condition-A-A-DOWN.POLYMARKET")),
                BookSubscriptionEvent::subscribe(InstrumentId::from("condition-B-B-UP.POLYMARKET")),
                BookSubscriptionEvent::subscribe(InstrumentId::from("condition-B-B-DOWN.POLYMARKET")),
            ]
        );
        drop(actor_ref);

        unsafe {
            DataActor::on_stop(&mut *actor_rc.get())
                .expect("registered actor should unsubscribe cleanly");
        }
        get_actor_registry().remove(&actor_id);
    }

    #[test]
    fn warmup_requires_consecutive_fresh_ticks() {
        let mut strategy = test_strategy();
        strategy.config.warmup_tick_count = 3;
        strategy.apply_selection_snapshot(active_snapshot("MKT-1"));

        strategy.observe_reference_snapshot(&reference_tick(1_000, 3_100.0));
        strategy.observe_reference_snapshot(&reference_tick(1_100, 3_101.0));
        assert!(!strategy.active.warmup_complete());

        strategy.observe_reference_snapshot(&reference_tick(1_200, 3_102.0));
        assert!(strategy.active.warmup_complete());
    }
}
