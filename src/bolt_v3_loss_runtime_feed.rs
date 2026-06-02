use std::{collections::VecDeque, sync::Arc};

use nautilus_common::msgbus::{
    TypedHandler, subscribe_portfolio_snapshot, subscribe_position_events,
    unsubscribe_portfolio_snapshot, unsubscribe_position_events,
};
use nautilus_model::{
    events::{PortfolioSnapshot, PositionEvent},
    identifiers::AccountId,
    types::{Currency, Money},
};
use rust_decimal::Decimal;

use crate::{
    bolt_v3_loss_governor::LossSnapshot,
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
    nt_runtime_capture::{portfolio_snapshots_pattern, position_events_pattern},
};

const LOSS_RUNTIME_FEED_SOURCE: &str = stringify!(nt_loss_runtime_feed);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LossGovernorRuntimeFeedConfig {
    pub account_id: AccountId,
    pub rolling_window_ns: u64,
}

#[derive(Debug)]
pub struct LossGovernorRuntimeFeed {
    config: LossGovernorRuntimeFeedConfig,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    state: LossGovernorRuntimeFeedState,
}

pub struct LossGovernorRuntimeFeedSubscription {
    position_events: Option<TypedHandler<PositionEvent>>,
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
    current_equity: Option<TimedDecimal>,
    peak_equity: Option<TimedDecimal>,
    latest_observed_at_ns: Option<u64>,
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

#[must_use]
pub fn subscribe_loss_governor_runtime_feed(
    feed: Arc<std::sync::Mutex<LossGovernorRuntimeFeed>>,
) -> LossGovernorRuntimeFeedSubscription {
    let position_feed = Arc::clone(&feed);
    let position_events = TypedHandler::from(move |event: &PositionEvent| {
        position_feed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .on_position_event(event);
    });
    subscribe_position_events(position_events_pattern(), position_events.clone(), None);

    let portfolio_feed = Arc::clone(&feed);
    let portfolio_snapshots = TypedHandler::from(move |snapshot: &PortfolioSnapshot| {
        portfolio_feed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .on_portfolio_snapshot(snapshot);
    });
    subscribe_portfolio_snapshot(
        portfolio_snapshots_pattern(),
        portfolio_snapshots.clone(),
        None,
    );

    LossGovernorRuntimeFeedSubscription {
        position_events: Some(position_events),
        portfolio_snapshots: Some(portfolio_snapshots),
    }
}

impl LossGovernorRuntimeFeedSubscription {
    pub fn unsubscribe_all(&mut self) {
        if let Some(position_events) = self.position_events.take() {
            unsubscribe_position_events(position_events_pattern(), &position_events);
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
            state: LossGovernorRuntimeFeedState::new(),
        }
    }

    pub fn on_portfolio_snapshot(&mut self, snapshot: &PortfolioSnapshot) -> Option<LossSnapshot> {
        if snapshot.account_id != self.config.account_id {
            return None;
        }

        let currency = portfolio_currency(snapshot)?;
        if !self.state.accept_currency(currency) {
            return None;
        }

        let observed_at_ns = snapshot.ts_event.as_u64();
        let daily_pnl = daily_pnl(snapshot, currency)?;
        let current_equity = total_equity(snapshot, currency)?;
        self.state.update_rolling_pnl(
            daily_pnl,
            current_equity,
            observed_at_ns,
            self.config.rolling_window_ns,
        );
        self.state.daily_pnl = Some(TimedDecimal::new(daily_pnl, observed_at_ns));
        self.state
            .per_trade_pnl
            .get_or_insert(TimedDecimal::new(Decimal::ZERO, observed_at_ns));

        self.state.current_equity = Some(TimedDecimal::new(current_equity, observed_at_ns));
        match self.state.peak_equity {
            Some(peak) if peak.value > current_equity => {}
            _ => {
                self.state.peak_equity = Some(TimedDecimal::new(current_equity, observed_at_ns));
            }
        }
        self.state.latest_observed_at_ns = Some(observed_at_ns);

        self.publish_if_complete()
    }

    pub fn on_position_event(&mut self, event: &PositionEvent) -> Option<LossSnapshot> {
        let position_fact = position_pnl_fact(event)?;
        if position_fact.account_id != self.config.account_id {
            return None;
        }
        if !self.state.accept_currency(position_fact.currency) {
            return None;
        }

        if let Some(per_trade_pnl) = position_fact.per_trade_pnl {
            self.state.per_trade_pnl = Some(TimedDecimal::new(
                per_trade_pnl,
                position_fact.observed_at_ns,
            ));
        }
        self.state.latest_observed_at_ns = Some(position_fact.observed_at_ns);

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
        let observed_at_ns = self.state.latest_observed_at_ns?;

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
}

impl LossGovernorRuntimeFeedState {
    const fn new() -> Self {
        Self {
            currency: None,
            previous_daily_pnl: None,
            previous_total_equity: None,
            daily_pnl: None,
            rolling_samples: VecDeque::new(),
            rolling_pnl: None,
            per_trade_pnl: None,
            current_equity: None,
            peak_equity: None,
            latest_observed_at_ns: None,
            latest_snapshot: None,
        }
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
                Some(previous_total_equity)
                    if cumulative_delta != daily_pnl
                        && current_equity - previous_total_equity.value == daily_pnl =>
                {
                    daily_pnl
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
    currency: Currency,
    per_trade_pnl: Option<Decimal>,
    observed_at_ns: u64,
}

fn position_pnl_fact(event: &PositionEvent) -> Option<PositionPnlFact> {
    match event {
        PositionEvent::PositionOpened(_) => None,
        PositionEvent::PositionChanged(changed) => Some(PositionPnlFact {
            account_id: changed.account_id,
            currency: changed.unrealized_pnl.currency,
            per_trade_pnl: Some(combined_position_pnl(
                changed.realized_pnl,
                changed.unrealized_pnl,
            )?),
            observed_at_ns: changed.ts_event.as_u64(),
        }),
        PositionEvent::PositionClosed(closed) => Some(PositionPnlFact {
            account_id: closed.account_id,
            currency: closed.unrealized_pnl.currency,
            per_trade_pnl: Some(combined_position_pnl(
                closed.realized_pnl,
                closed.unrealized_pnl,
            )?),
            observed_at_ns: closed.ts_event.as_u64(),
        }),
        PositionEvent::PositionAdjusted(adjusted) => {
            let pnl_change = adjusted.pnl_change?;
            Some(PositionPnlFact {
                account_id: adjusted.account_id,
                currency: pnl_change.currency,
                per_trade_pnl: None,
                observed_at_ns: adjusted.ts_event.as_u64(),
            })
        }
    }
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

fn sum_money<'a>(values: impl Iterator<Item = &'a Money>, currency: Currency) -> Option<Decimal> {
    let mut found = false;
    let mut total = Decimal::ZERO;
    for value in values.filter(|value| value.currency == currency) {
        found = true;
        total += value.as_decimal();
    }
    found.then_some(total)
}
