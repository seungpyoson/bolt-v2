use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    fmt,
    rc::Rc,
    sync::Arc,
};

use nautilus_common::msgbus::{
    TypedHandler, subscribe_account_state, subscribe_portfolio_snapshot, subscribe_position_events,
    unsubscribe_account_state, unsubscribe_portfolio_snapshot, unsubscribe_position_events,
};
use nautilus_model::{
    events::{AccountState, PortfolioSnapshot, PositionEvent},
    identifiers::{AccountId, PositionId},
    types::{Currency, Money},
};
use rust_decimal::Decimal;

use crate::{
    bolt_v3_loss_governor::LossSnapshot,
    bolt_v3_loss_halt_actions::LossGovernorHaltActionHandler,
    bolt_v3_submit_admission::{BoltV3LossFreshness, BoltV3SubmitAdmissionState},
    nt_runtime_capture::{
        account_states_pattern, portfolio_snapshots_pattern, position_events_pattern,
    },
};

const LOSS_RUNTIME_FEED_SOURCE: &str = stringify!(nt_loss_runtime_feed);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LossGovernorRuntimeFeedConfig {
    pub account_id: AccountId,
    pub rolling_window_ns: u64,
    pub active_position_pnl_max_entries: usize,
}

pub struct LossGovernorRuntimeFeed {
    config: LossGovernorRuntimeFeedConfig,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    halt_action_handler: Option<LossGovernorHaltActionHandler>,
    state: LossGovernorRuntimeFeedState,
}

pub struct LossGovernorRuntimeFeedSubscription {
    position_events: Option<TypedHandler<PositionEvent>>,
    account_states: Option<TypedHandler<AccountState>>,
    portfolio_snapshots: Option<TypedHandler<PortfolioSnapshot>>,
}

#[derive(Debug)]
struct LossGovernorRuntimeFeedState {
    currency: Option<Currency>,
    previous_daily_pnl: Option<TimedDecimal>,
    previous_total_equity: Option<TimedDecimal>,
    daily_pnl: Option<TimedDecimal>,
    rolling_samples: VecDeque<TimedDecimal>,
    rolling_pnl: Option<TimedDecimal>,
    per_trade_pnl: Option<TimedDecimal>,
    per_trade_pnl_source: Option<PerTradePnlSource>,
    active_position_pnls: BTreeMap<PositionId, TimedDecimal>,
    active_position_pnl_overflow_floor: Option<TimedDecimal>,
    last_position_pnl: Option<TimedDecimal>,
    current_equity: Option<TimedDecimal>,
    peak_equity: Option<TimedDecimal>,
    account_state_equity_baseline: Option<TimedDecimal>,
    portfolio_pnl_observed: bool,
    account_state_count: u64,
    portfolio_snapshot_count: u64,
    position_event_count: u64,
    last_account_state_ts_ns: Option<u64>,
    last_portfolio_snapshot_ts_ns: Option<u64>,
    last_position_event_ts_ns: Option<u64>,
    latest_snapshot: Option<LossSnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TimedDecimal {
    value: Decimal,
    observed_at_ns: u64,
}

impl TimedDecimal {
    const fn new(value: Decimal, observed_at_ns: u64) -> Self {
        Self {
            value,
            observed_at_ns,
        }
    }
}

fn worse_timed_decimal(current: Option<TimedDecimal>, candidate: TimedDecimal) -> TimedDecimal {
    match current {
        Some(current) if current.value <= candidate.value => current,
        _ => candidate,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PerTradePnlSource {
    PortfolioBaseline,
    PositionEvent,
}

#[must_use]
pub fn subscribe_loss_governor_runtime_feed(
    feed: Rc<RefCell<LossGovernorRuntimeFeed>>,
) -> LossGovernorRuntimeFeedSubscription {
    let position_feed = Rc::clone(&feed);
    let position_events = TypedHandler::from(move |event: &PositionEvent| {
        let now_ns = position_event_ts_init(event);
        let (snapshot, handler, should_invoke) = {
            let mut feed = position_feed.borrow_mut();
            let should_invoke = position_event_can_carry_loss_fact(event)
                && position_event_account_id(event) == feed.config.account_id;
            (
                should_invoke
                    .then(|| feed.on_position_event_without_halt_action(event))
                    .flatten(),
                feed.halt_action_handler.clone(),
                should_invoke,
            )
        };
        if should_invoke {
            invoke_loss_halt_action(handler, snapshot.as_ref(), now_ns);
        }
    });
    subscribe_position_events(position_events_pattern(), position_events.clone(), None);

    let account_feed = Rc::clone(&feed);
    let account_states = TypedHandler::from(move |state: &AccountState| {
        let now_ns = state.ts_init.as_u64();
        let (loss_snapshot, handler, should_invoke) = {
            let mut feed = account_feed.borrow_mut();
            let should_invoke = state.account_id == feed.config.account_id;
            (
                should_invoke
                    .then(|| feed.on_account_state_without_halt_action(state))
                    .flatten(),
                feed.halt_action_handler.clone(),
                should_invoke,
            )
        };
        if should_invoke {
            invoke_loss_halt_action(handler, loss_snapshot.as_ref(), now_ns);
        }
    });
    subscribe_account_state(account_states_pattern(), account_states.clone(), None);

    let portfolio_feed = Rc::clone(&feed);
    let portfolio_snapshots = TypedHandler::from(move |snapshot: &PortfolioSnapshot| {
        let now_ns = snapshot.ts_init.as_u64();
        let (loss_snapshot, handler, should_invoke) = {
            let mut feed = portfolio_feed.borrow_mut();
            let should_invoke = snapshot.account_id == feed.config.account_id;
            (
                should_invoke
                    .then(|| feed.on_portfolio_snapshot_without_halt_action(snapshot))
                    .flatten(),
                feed.halt_action_handler.clone(),
                should_invoke,
            )
        };
        if should_invoke {
            invoke_loss_halt_action(handler, loss_snapshot.as_ref(), now_ns);
        }
    });
    subscribe_portfolio_snapshot(
        portfolio_snapshots_pattern(),
        portfolio_snapshots.clone(),
        None,
    );

    LossGovernorRuntimeFeedSubscription {
        position_events: Some(position_events),
        account_states: Some(account_states),
        portfolio_snapshots: Some(portfolio_snapshots),
    }
}

impl LossGovernorRuntimeFeedSubscription {
    pub fn unsubscribe_all(&mut self) {
        if let Some(position_events) = self.position_events.take() {
            unsubscribe_position_events(position_events_pattern(), &position_events);
        }
        if let Some(account_states) = self.account_states.take() {
            unsubscribe_account_state(account_states_pattern(), &account_states);
        }
        if let Some(portfolio_snapshots) = self.portfolio_snapshots.take() {
            unsubscribe_portfolio_snapshot(portfolio_snapshots_pattern(), &portfolio_snapshots);
        }
    }
}

impl Drop for LossGovernorRuntimeFeedSubscription {
    fn drop(&mut self) {
        self.unsubscribe_all();
    }
}

impl LossGovernorRuntimeFeed {
    #[must_use]
    pub fn new(
        config: LossGovernorRuntimeFeedConfig,
        submit_admission: Arc<BoltV3SubmitAdmissionState>,
    ) -> Self {
        Self {
            config,
            submit_admission,
            halt_action_handler: None,
            state: LossGovernorRuntimeFeedState::new(),
        }
    }

    #[must_use]
    pub fn with_halt_action_handler(mut self, handler: LossGovernorHaltActionHandler) -> Self {
        self.halt_action_handler = Some(handler);
        self
    }

    pub fn on_portfolio_snapshot(&mut self, snapshot: &PortfolioSnapshot) -> Option<LossSnapshot> {
        if snapshot.account_id != self.config.account_id {
            return None;
        }
        let now_ns = snapshot.ts_init.as_u64();
        let snapshot = self.on_portfolio_snapshot_without_halt_action(snapshot);
        self.invoke_halt_action(snapshot.as_ref(), now_ns);
        snapshot
    }

    fn on_portfolio_snapshot_without_halt_action(
        &mut self,
        snapshot: &PortfolioSnapshot,
    ) -> Option<LossSnapshot> {
        if snapshot.account_id != self.config.account_id {
            return None;
        }

        let observed_at_ns = snapshot.ts_event.as_u64();
        self.state
            .record_portfolio_snapshot_freshness(observed_at_ns);
        self.submit_admission
            .update_loss_freshness(self.state.freshness());

        let currency = portfolio_currency(snapshot)?;
        if !self.state.accept_currency(currency) {
            return None;
        }

        let daily_pnl = daily_pnl(snapshot, currency)?;
        let current_equity = total_equity(snapshot, currency)?;
        self.state.portfolio_pnl_observed = true;
        self.state.update_rolling_pnl(
            daily_pnl,
            current_equity,
            observed_at_ns,
            self.config.rolling_window_ns,
        );
        self.state.daily_pnl = Some(TimedDecimal::new(daily_pnl, observed_at_ns));
        self.state.refresh_per_trade_pnl_clock(observed_at_ns);
        self.state.current_equity = Some(TimedDecimal::new(current_equity, observed_at_ns));
        self.state
            .preserve_or_raise_peak_equity(current_equity, observed_at_ns);

        self.publish_if_complete()
    }

    pub fn on_account_state(&mut self, state: &AccountState) -> Option<LossSnapshot> {
        if state.account_id != self.config.account_id {
            return None;
        }
        let now_ns = state.ts_init.as_u64();
        let snapshot = self.on_account_state_without_halt_action(state);
        self.invoke_halt_action(snapshot.as_ref(), now_ns);
        snapshot
    }

    fn on_account_state_without_halt_action(
        &mut self,
        state: &AccountState,
    ) -> Option<LossSnapshot> {
        if state.account_id != self.config.account_id {
            return None;
        }

        let observed_at_ns = state.ts_event.as_u64();
        self.state.record_account_state_freshness(observed_at_ns);
        self.submit_admission
            .update_loss_freshness(self.state.freshness());

        let currency = account_currency(state)?;
        if !self.state.accept_currency(currency) {
            return None;
        }

        let current_equity = account_total_equity(state, currency)?;
        if self.state.portfolio_pnl_observed {
            let daily_pnl = self
                .state
                .daily_pnl
                .map_or(Decimal::ZERO, |daily_pnl| daily_pnl.value);
            let rolling_pnl = self
                .state
                .rolling_pnl
                .map_or(Decimal::ZERO, |rolling_pnl| rolling_pnl.value);
            self.state.daily_pnl = Some(TimedDecimal::new(daily_pnl, observed_at_ns));
            self.state.rolling_pnl = Some(TimedDecimal::new(rolling_pnl, observed_at_ns));
            self.state.previous_daily_pnl = Some(TimedDecimal::new(daily_pnl, observed_at_ns));
            self.state.previous_total_equity =
                Some(TimedDecimal::new(current_equity, observed_at_ns));
            self.state.refresh_per_trade_pnl_clock(observed_at_ns);
            self.state.current_equity = Some(TimedDecimal::new(current_equity, observed_at_ns));
            self.state
                .preserve_or_raise_peak_equity(current_equity, observed_at_ns);

            return self.publish_if_complete();
        }

        let daily_pnl = {
            let baseline = self
                .state
                .account_state_equity_baseline
                .get_or_insert(TimedDecimal::new(current_equity, observed_at_ns));
            current_equity - baseline.value
        };
        self.state.update_rolling_pnl(
            daily_pnl,
            current_equity,
            observed_at_ns,
            self.config.rolling_window_ns,
        );
        self.state.daily_pnl = Some(TimedDecimal::new(daily_pnl, observed_at_ns));
        self.state.refresh_per_trade_pnl_clock(observed_at_ns);
        self.state.current_equity = Some(TimedDecimal::new(current_equity, observed_at_ns));
        self.state
            .preserve_or_raise_peak_equity(current_equity, observed_at_ns);

        self.publish_if_complete()
    }

    pub fn on_position_event(&mut self, event: &PositionEvent) -> Option<LossSnapshot> {
        if !position_event_can_carry_loss_fact(event)
            || position_event_account_id(event) != self.config.account_id
        {
            return None;
        }
        let now_ns = position_event_ts_init(event);
        let snapshot = self.on_position_event_without_halt_action(event);
        self.invoke_halt_action(snapshot.as_ref(), now_ns);
        snapshot
    }

    fn on_position_event_without_halt_action(
        &mut self,
        event: &PositionEvent,
    ) -> Option<LossSnapshot> {
        if position_event_account_id(event) != self.config.account_id {
            return None;
        }

        let observed_at_ns = position_event_ts_event(event);
        self.state.record_position_event_freshness(observed_at_ns);
        self.submit_admission
            .update_loss_freshness(self.state.freshness());

        let position_fact = position_pnl_fact(event)?;
        if position_fact.account_id != self.config.account_id {
            return None;
        }
        if !self.state.accept_currency(position_fact.currency) {
            return None;
        }

        if let Some(per_trade_pnl) = position_fact.per_trade_pnl {
            self.state.record_position_pnl(
                position_fact.position_id,
                TimedDecimal::new(per_trade_pnl, position_fact.observed_at_ns),
                position_fact.closed,
                position_fact.reset_completed_position_pnl,
                self.config.active_position_pnl_max_entries,
            );
        }

        self.publish_if_complete()
    }

    #[must_use]
    pub fn latest_snapshot(&self) -> Option<&LossSnapshot> {
        self.state.latest_snapshot.as_ref()
    }

    fn publish_if_complete(&mut self) -> Option<LossSnapshot> {
        let per_trade_pnl = self.state.per_trade_pnl?;
        let daily_pnl = self.state.daily_pnl?;
        let rolling_pnl = self.state.rolling_pnl?;
        let current_equity = self.state.current_equity?;
        let peak_equity = self.state.peak_equity?;
        let observed_at_ns = per_trade_pnl
            .observed_at_ns
            .min(daily_pnl.observed_at_ns)
            .min(rolling_pnl.observed_at_ns)
            .min(current_equity.observed_at_ns);

        let snapshot = LossSnapshot {
            source: LOSS_RUNTIME_FEED_SOURCE.to_string(),
            observed_at_ns,
            per_trade_pnl: Some(per_trade_pnl.value),
            daily_pnl: Some(daily_pnl.value),
            rolling_pnl: Some(rolling_pnl.value),
            current_equity: Some(current_equity.value),
            peak_equity: Some(peak_equity.value),
        };
        self.submit_admission.update_loss_snapshot(snapshot.clone());
        self.state.latest_snapshot = Some(snapshot.clone());
        Some(snapshot)
    }

    fn invoke_halt_action(&self, snapshot: Option<&LossSnapshot>, now_ns: u64) {
        if let Some(handler) = self.halt_action_handler.as_ref() {
            handler(snapshot, now_ns);
        }
    }
}

fn invoke_loss_halt_action(
    handler: Option<LossGovernorHaltActionHandler>,
    snapshot: Option<&LossSnapshot>,
    now_ns: u64,
) {
    if let Some(handler) = handler {
        handler(snapshot, now_ns);
    }
}

impl fmt::Debug for LossGovernorRuntimeFeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LossGovernorRuntimeFeed")
            .field("config", &self.config)
            .field("submit_admission", &self.submit_admission)
            .field(
                stringify!(halt_action_handler_configured),
                &self.halt_action_handler.is_some(),
            )
            .field("state", &self.state)
            .finish()
    }
}

impl LossGovernorRuntimeFeedState {
    fn new() -> Self {
        Self {
            currency: None,
            previous_daily_pnl: None,
            previous_total_equity: None,
            daily_pnl: None,
            rolling_samples: VecDeque::new(),
            rolling_pnl: None,
            per_trade_pnl: None,
            per_trade_pnl_source: None,
            active_position_pnls: BTreeMap::new(),
            active_position_pnl_overflow_floor: None,
            last_position_pnl: None,
            current_equity: None,
            peak_equity: None,
            account_state_equity_baseline: None,
            portfolio_pnl_observed: false,
            account_state_count: 0,
            portfolio_snapshot_count: 0,
            position_event_count: 0,
            last_account_state_ts_ns: None,
            last_portfolio_snapshot_ts_ns: None,
            last_position_event_ts_ns: None,
            latest_snapshot: None,
        }
    }

    fn freshness(&self) -> BoltV3LossFreshness {
        BoltV3LossFreshness {
            account_state_count: self.account_state_count,
            portfolio_snapshot_count: self.portfolio_snapshot_count,
            position_event_count: self.position_event_count,
            last_account_state_ts_ns: self.last_account_state_ts_ns,
            last_portfolio_snapshot_ts_ns: self.last_portfolio_snapshot_ts_ns,
            last_position_event_ts_ns: self.last_position_event_ts_ns,
        }
    }

    fn record_account_state_freshness(&mut self, observed_at_ns: u64) {
        self.account_state_count = self.account_state_count.saturating_add(1);
        self.last_account_state_ts_ns = Some(observed_at_ns);
    }

    fn record_portfolio_snapshot_freshness(&mut self, observed_at_ns: u64) {
        self.portfolio_snapshot_count = self.portfolio_snapshot_count.saturating_add(1);
        self.last_portfolio_snapshot_ts_ns = Some(observed_at_ns);
    }

    fn record_position_event_freshness(&mut self, observed_at_ns: u64) {
        self.position_event_count = self.position_event_count.saturating_add(1);
        self.last_position_event_ts_ns = Some(observed_at_ns);
    }

    fn accept_currency(&mut self, currency: Currency) -> bool {
        match self.currency {
            Some(existing) => existing == currency,
            None => {
                self.currency = Some(currency);
                true
            }
        }
    }

    fn refresh_per_trade_pnl_clock(&mut self, observed_at_ns: u64) {
        if self.per_trade_pnl_source == Some(PerTradePnlSource::PositionEvent) {
            for per_trade_pnl in self.active_position_pnls.values_mut() {
                *per_trade_pnl = TimedDecimal::new(per_trade_pnl.value, observed_at_ns);
            }
            if let Some(overflow_floor) = self.active_position_pnl_overflow_floor {
                self.active_position_pnl_overflow_floor =
                    Some(TimedDecimal::new(overflow_floor.value, observed_at_ns));
            }
            if let Some(last_position_pnl) = self.last_position_pnl {
                self.last_position_pnl =
                    Some(TimedDecimal::new(last_position_pnl.value, observed_at_ns));
            }
            self.refresh_effective_per_trade_pnl();
        } else {
            self.per_trade_pnl = Some(TimedDecimal::new(Decimal::ZERO, observed_at_ns));
            self.per_trade_pnl_source = Some(PerTradePnlSource::PortfolioBaseline);
        }
    }

    fn preserve_or_raise_peak_equity(&mut self, current_equity: Decimal, observed_at_ns: u64) {
        match self.peak_equity {
            Some(peak) if peak.value > current_equity => {}
            _ => {
                self.peak_equity = Some(TimedDecimal::new(current_equity, observed_at_ns));
            }
        }
    }

    fn record_position_pnl(
        &mut self,
        position_id: PositionId,
        position_pnl: TimedDecimal,
        closed: bool,
        reset_completed_position_pnl: bool,
        active_position_pnl_max_entries: usize,
    ) {
        if reset_completed_position_pnl {
            self.last_position_pnl = None;
        }
        if closed {
            self.active_position_pnls.remove(&position_id);
        } else if self.active_position_pnls.contains_key(&position_id)
            || self.active_position_pnls.len() < active_position_pnl_max_entries
        {
            self.active_position_pnls.insert(position_id, position_pnl);
        } else {
            self.active_position_pnl_overflow_floor = Some(worse_timed_decimal(
                self.active_position_pnl_overflow_floor,
                position_pnl,
            ));
        }
        if !reset_completed_position_pnl {
            self.last_position_pnl = Some(position_pnl);
        }
        self.refresh_effective_per_trade_pnl();
    }

    fn refresh_effective_per_trade_pnl(&mut self) {
        let worst_position_pnl = self
            .active_position_pnls
            .values()
            .copied()
            .chain(self.active_position_pnl_overflow_floor)
            .chain(self.last_position_pnl)
            .min_by_key(|per_trade_pnl| per_trade_pnl.value);
        if let Some(worst_position_pnl) = worst_position_pnl {
            self.per_trade_pnl = Some(worst_position_pnl);
            self.per_trade_pnl_source = Some(PerTradePnlSource::PositionEvent);
        }
    }

    fn update_rolling_pnl(
        &mut self,
        daily_pnl: Decimal,
        current_equity: Decimal,
        observed_at_ns: u64,
        rolling_window_ns: u64,
    ) {
        if let Some(previous_daily_pnl) = self.previous_daily_pnl {
            let cumulative_delta = daily_pnl - previous_daily_pnl.value;
            let rolling_delta = match self.previous_total_equity {
                Some(previous_total_equity) => {
                    let equity_delta = current_equity - previous_total_equity.value;
                    if cumulative_delta != daily_pnl && equity_delta == daily_pnl {
                        daily_pnl
                    } else {
                        cumulative_delta
                    }
                }
                _ => cumulative_delta,
            };
            self.rolling_samples
                .push_back(TimedDecimal::new(rolling_delta, observed_at_ns));
        }
        self.previous_daily_pnl = Some(TimedDecimal::new(daily_pnl, observed_at_ns));
        self.previous_total_equity = Some(TimedDecimal::new(current_equity, observed_at_ns));

        let oldest_accepted = observed_at_ns.saturating_sub(rolling_window_ns);
        while self
            .rolling_samples
            .front()
            .is_some_and(|sample| sample.observed_at_ns < oldest_accepted)
        {
            self.rolling_samples.pop_front();
        }

        let rolling_pnl = self.rolling_samples.iter().map(|sample| sample.value).sum();
        self.rolling_pnl = Some(TimedDecimal::new(rolling_pnl, observed_at_ns));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PositionPnlFact {
    account_id: AccountId,
    position_id: PositionId,
    currency: Currency,
    per_trade_pnl: Option<Decimal>,
    observed_at_ns: u64,
    closed: bool,
    reset_completed_position_pnl: bool,
}

fn position_pnl_fact(event: &PositionEvent) -> Option<PositionPnlFact> {
    match event {
        PositionEvent::PositionOpened(opened) => Some(PositionPnlFact {
            account_id: opened.account_id,
            position_id: opened.position_id,
            currency: opened.currency,
            per_trade_pnl: Some(Decimal::ZERO),
            observed_at_ns: opened.ts_event.as_u64(),
            closed: false,
            reset_completed_position_pnl: true,
        }),
        PositionEvent::PositionChanged(changed) => Some(PositionPnlFact {
            account_id: changed.account_id,
            position_id: changed.position_id,
            currency: changed.unrealized_pnl.currency,
            per_trade_pnl: Some(combined_position_pnl(
                changed.realized_pnl,
                changed.unrealized_pnl,
            )?),
            observed_at_ns: changed.ts_event.as_u64(),
            closed: false,
            reset_completed_position_pnl: false,
        }),
        PositionEvent::PositionClosed(closed) => Some(PositionPnlFact {
            account_id: closed.account_id,
            position_id: closed.position_id,
            currency: closed.unrealized_pnl.currency,
            per_trade_pnl: Some(combined_position_pnl(
                closed.realized_pnl,
                closed.unrealized_pnl,
            )?),
            observed_at_ns: closed.ts_event.as_u64(),
            closed: true,
            reset_completed_position_pnl: false,
        }),
        PositionEvent::PositionAdjusted(adjusted) => {
            let pnl_change = adjusted.pnl_change?;
            Some(PositionPnlFact {
                account_id: adjusted.account_id,
                position_id: adjusted.position_id,
                currency: pnl_change.currency,
                per_trade_pnl: None,
                observed_at_ns: adjusted.ts_event.as_u64(),
                closed: false,
                reset_completed_position_pnl: false,
            })
        }
    }
}

fn position_event_account_id(event: &PositionEvent) -> AccountId {
    match event {
        PositionEvent::PositionOpened(opened) => opened.account_id,
        PositionEvent::PositionChanged(changed) => changed.account_id,
        PositionEvent::PositionClosed(closed) => closed.account_id,
        PositionEvent::PositionAdjusted(adjusted) => adjusted.account_id,
    }
}

fn position_event_ts_init(event: &PositionEvent) -> u64 {
    match event {
        PositionEvent::PositionOpened(opened) => opened.ts_init.as_u64(),
        PositionEvent::PositionChanged(changed) => changed.ts_init.as_u64(),
        PositionEvent::PositionClosed(closed) => closed.ts_init.as_u64(),
        PositionEvent::PositionAdjusted(adjusted) => adjusted.ts_init.as_u64(),
    }
}

fn position_event_ts_event(event: &PositionEvent) -> u64 {
    match event {
        PositionEvent::PositionOpened(opened) => opened.ts_event.as_u64(),
        PositionEvent::PositionChanged(changed) => changed.ts_event.as_u64(),
        PositionEvent::PositionClosed(closed) => closed.ts_event.as_u64(),
        PositionEvent::PositionAdjusted(adjusted) => adjusted.ts_event.as_u64(),
    }
}

fn position_event_can_carry_loss_fact(event: &PositionEvent) -> bool {
    matches!(
        event,
        PositionEvent::PositionOpened(_)
            | PositionEvent::PositionChanged(_)
            | PositionEvent::PositionClosed(_)
            | PositionEvent::PositionAdjusted(_)
    )
}

fn combined_position_pnl(realized_pnl: Option<Money>, unrealized_pnl: Money) -> Option<Decimal> {
    match realized_pnl {
        Some(realized_pnl) if realized_pnl.currency == unrealized_pnl.currency => {
            Some(realized_pnl.as_decimal() + unrealized_pnl.as_decimal())
        }
        Some(_) => None,
        None => Some(unrealized_pnl.as_decimal()),
    }
}

fn portfolio_currency(snapshot: &PortfolioSnapshot) -> Option<Currency> {
    snapshot
        .base_currency
        .or_else(|| single_money_currency(snapshot))
}

fn account_currency(state: &AccountState) -> Option<Currency> {
    state
        .base_currency
        .or_else(|| single_account_balance_currency(state))
}

fn single_account_balance_currency(state: &AccountState) -> Option<Currency> {
    let mut currency = None;
    for balance in &state.balances {
        match currency {
            Some(existing) if existing != balance.total.currency => return None,
            Some(_) => {}
            None => currency = Some(balance.total.currency),
        }
    }
    currency
}

fn single_money_currency(snapshot: &PortfolioSnapshot) -> Option<Currency> {
    let mut currency = None;
    for money in snapshot
        .unrealized_pnls
        .iter()
        .chain(snapshot.realized_pnls.iter())
        .chain(snapshot.total_equity.iter())
    {
        match currency {
            Some(existing) if existing != money.currency => return None,
            Some(_) => {}
            None => currency = Some(money.currency),
        }
    }
    currency
}

fn daily_pnl(snapshot: &PortfolioSnapshot, currency: Currency) -> Option<Decimal> {
    sum_money(
        snapshot
            .realized_pnls
            .iter()
            .chain(snapshot.unrealized_pnls.iter()),
        currency,
    )
}

fn total_equity(snapshot: &PortfolioSnapshot, currency: Currency) -> Option<Decimal> {
    sum_money(snapshot.total_equity.iter(), currency)
}

fn account_total_equity(state: &AccountState, currency: Currency) -> Option<Decimal> {
    let mut found = false;
    let mut total = Decimal::ZERO;
    for balance in state
        .balances
        .iter()
        .filter(|balance| balance.total.currency == currency)
    {
        found = true;
        total += balance.total.as_decimal();
    }
    found.then_some(total)
}

fn sum_money<'a>(values: impl Iterator<Item = &'a Money>, currency: Currency) -> Option<Decimal> {
    let mut found = false;
    let mut total = Decimal::ZERO;
    for value in values.filter(|value| value.currency == currency) {
        found = true;
        total += value.as_decimal();
    }
    found.then_some(total)
}

#[cfg(test)]
mod tests {
    use super::{LossGovernorRuntimeFeedState, TimedDecimal};
    use nautilus_model::identifiers::PositionId;
    use rust_decimal::Decimal;

    #[test]
    fn active_position_pnl_cap_keeps_bounded_map_and_conservative_overflow_floor() {
        let mut state = LossGovernorRuntimeFeedState::new();

        state.record_position_pnl(
            PositionId::from("POSITION-A"),
            TimedDecimal::new(Decimal::new(-8, 0), 1_000),
            false,
            false,
            1,
        );
        state.record_position_pnl(
            PositionId::from("POSITION-B"),
            TimedDecimal::new(Decimal::new(-1, 0), 1_100),
            false,
            false,
            1,
        );
        assert_eq!(state.active_position_pnls.len(), 1);
        assert_eq!(
            state.active_position_pnl_overflow_floor,
            Some(TimedDecimal::new(Decimal::new(-1, 0), 1_100))
        );
        assert_eq!(
            state.per_trade_pnl,
            Some(TimedDecimal::new(Decimal::new(-8, 0), 1_000))
        );

        state.record_position_pnl(
            PositionId::from("POSITION-C"),
            TimedDecimal::new(Decimal::new(-12, 0), 1_200),
            false,
            false,
            1,
        );

        assert_eq!(state.active_position_pnls.len(), 1);
        assert_eq!(
            state.active_position_pnl_overflow_floor,
            Some(TimedDecimal::new(Decimal::new(-12, 0), 1_200))
        );
        assert_eq!(
            state.per_trade_pnl,
            Some(TimedDecimal::new(Decimal::new(-12, 0), 1_200))
        );
    }
}
