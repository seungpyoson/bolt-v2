use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    fmt,
    rc::Rc,
    sync::Arc,
};

use nautilus_common::msgbus::{
    TypedHandler, subscribe_portfolio_snapshot, subscribe_position_events,
    unsubscribe_portfolio_snapshot, unsubscribe_position_events,
};
use nautilus_model::{
    events::{PortfolioSnapshot, PositionEvent},
    identifiers::{AccountId, PositionId},
    types::{Currency, Money},
};
use rust_decimal::Decimal;

use crate::{
    bolt_v3_loss_governor::LossSnapshot,
    bolt_v3_loss_halt_actions::LossGovernorHaltActionHandler,
    bolt_v3_submit_admission::BoltV3SubmitAdmissionState,
    nt_runtime_capture::{portfolio_snapshots_pattern, position_events_pattern},
};

const LOSS_RUNTIME_FEED_SOURCE: &str = stringify!(nt_loss_runtime_feed);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LossGovernorRuntimeFeedConfig {
    pub account_id: AccountId,
    pub rolling_window_ns: u64,
}

pub struct LossGovernorRuntimeFeed {
    config: LossGovernorRuntimeFeedConfig,
    submit_admission: Arc<BoltV3SubmitAdmissionState>,
    halt_action_handler: Option<LossGovernorHaltActionHandler>,
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
    // Worst-of-N per-trade loss across concurrent OPEN positions: tracked per
    // `PositionId`, opened on `PositionOpened` (at zero) / updated on
    // `PositionChanged` / removed on `PositionClosed`. The published per-trade
    // figure is the most negative (min) leg across all open positions, so a
    // benign event on one position can never mask a larger loss on another.
    position_pnls: HashMap<PositionId, TimedDecimal>,
    // Zero-baseline per-trade fact established by a portfolio snapshot, used only
    // when there are no open positions to derive a worst-of-N figure from.
    portfolio_baseline_per_trade: Option<TimedDecimal>,
    current_equity: Option<TimedDecimal>,
    peak_equity: Option<TimedDecimal>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeakEquityAction {
    Preserve,
    RebaselineToCurrent,
}

#[must_use]
pub fn subscribe_loss_governor_runtime_feed(
    feed: Rc<RefCell<LossGovernorRuntimeFeed>>,
) -> LossGovernorRuntimeFeedSubscription {
    let position_feed = Rc::clone(&feed);
    let position_events = TypedHandler::from(move |event: &PositionEvent| {
        let (snapshot, handler) = {
            let mut feed = position_feed.borrow_mut();
            (
                feed.on_position_event_without_halt_action(event),
                feed.halt_action_handler.clone(),
            )
        };
        invoke_loss_halt_action(handler, snapshot.as_ref());
    });
    subscribe_position_events(position_events_pattern(), position_events.clone(), None);

    let portfolio_feed = Rc::clone(&feed);
    let portfolio_snapshots = TypedHandler::from(move |snapshot: &PortfolioSnapshot| {
        let (loss_snapshot, handler) = {
            let mut feed = portfolio_feed.borrow_mut();
            (
                feed.on_portfolio_snapshot_without_halt_action(snapshot),
                feed.halt_action_handler.clone(),
            )
        };
        invoke_loss_halt_action(handler, loss_snapshot.as_ref());
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
        let snapshot = self.on_portfolio_snapshot_without_halt_action(snapshot)?;
        self.invoke_halt_action(&snapshot);
        Some(snapshot)
    }

    fn on_portfolio_snapshot_without_halt_action(
        &mut self,
        snapshot: &PortfolioSnapshot,
    ) -> Option<LossSnapshot> {
        if snapshot.account_id != self.config.account_id {
            return None;
        }

        let observed_at_ns = snapshot.ts_event.as_u64();

        // Fail-closed contract: any corrupt/un-summable portfolio heartbeat
        // (unresolvable account currency, currency mismatch against the
        // established account currency, or money facts that cannot be summed)
        // must INVALIDATE the cached portfolio facts and publish a cleared,
        // all-None snapshot so the next admission trips `snapshot_is_stale`.
        // Returning `None` here would leave the prior good snapshot live and
        // keep admitting on stale facts.
        let Some(currency) = portfolio_currency(snapshot) else {
            return self.invalidate_portfolio_facts(observed_at_ns);
        };
        if !self.state.accept_currency(currency) {
            return self.invalidate_portfolio_facts(observed_at_ns);
        }
        let Some(daily_pnl) = daily_pnl(snapshot, currency) else {
            return self.invalidate_portfolio_facts(observed_at_ns);
        };
        let Some(current_equity) = total_equity(snapshot, currency) else {
            return self.invalidate_portfolio_facts(observed_at_ns);
        };

        let peak_equity_action = self.state.update_rolling_pnl(
            daily_pnl,
            current_equity,
            observed_at_ns,
            self.config.rolling_window_ns,
        );
        self.state.daily_pnl = Some(TimedDecimal::new(daily_pnl, observed_at_ns));
        // Seed the zero per-trade baseline used while no positions are open. The
        // worst-of-N derivation in `per_trade_pnl()` ignores this baseline as
        // soon as any position is being tracked, so the baseline can never mask
        // an open per-trade loss.
        self.state.portfolio_baseline_per_trade =
            Some(TimedDecimal::new(Decimal::ZERO, observed_at_ns));

        self.state.current_equity = Some(TimedDecimal::new(current_equity, observed_at_ns));
        match (peak_equity_action, self.state.peak_equity) {
            (PeakEquityAction::RebaselineToCurrent, _) => {
                self.state.peak_equity = Some(TimedDecimal::new(current_equity, observed_at_ns));
            }
            (PeakEquityAction::Preserve, Some(peak)) if peak.value > current_equity => {}
            (PeakEquityAction::Preserve, _) => {
                self.state.peak_equity = Some(TimedDecimal::new(current_equity, observed_at_ns));
            }
        }

        self.publish_if_complete()
    }

    pub fn on_position_event(&mut self, event: &PositionEvent) -> Option<LossSnapshot> {
        let snapshot = self.on_position_event_without_halt_action(event)?;
        self.invoke_halt_action(&snapshot);
        Some(snapshot)
    }

    fn on_position_event_without_halt_action(
        &mut self,
        event: &PositionEvent,
    ) -> Option<LossSnapshot> {
        let position_fact = position_pnl_fact(event)?;
        if position_fact.account_id != self.config.account_id {
            return None;
        }

        match position_fact.kind {
            // Open / change carry an absolute trade-level PnL for one position;
            // start tracking it (PositionOpened seeds zero) or update its value.
            // A currency that mismatches the established account currency is
            // dropped fail-closed (no per-position fact is recorded), matching
            // the portfolio path's currency discipline.
            PositionPnlKind::Absolute(per_trade_pnl) => {
                if !self.state.accept_currency(position_fact.currency) {
                    return None;
                }
                self.state.position_pnls.insert(
                    position_fact.position_id,
                    TimedDecimal::new(per_trade_pnl, position_fact.observed_at_ns),
                );
            }
            // Close removes the leg from the worst-of-N set; the per-trade figure
            // falls back to the next-worst open position, or the zero baseline.
            PositionPnlKind::Closed => {
                self.state.position_pnls.remove(&position_fact.position_id);
            }
            // Adjustments (e.g. commission deltas) are not a trade-level PnL
            // source: they must never overwrite the worst-of-open selection.
            PositionPnlKind::Adjustment => {}
        }

        self.publish_if_complete()
    }

    #[must_use]
    pub fn latest_snapshot(&self) -> Option<&LossSnapshot> {
        self.state.latest_snapshot.as_ref()
    }

    fn publish_if_complete(&mut self) -> Option<LossSnapshot> {
        let per_trade_pnl = self.state.per_trade_pnl()?;
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

    /// Invalidate cached portfolio facts on a corrupt/un-summable heartbeat and
    /// publish a cleared, all-None snapshot so the next admission fails closed.
    ///
    /// Returns `None` (publishing nothing) only when no good snapshot has ever
    /// been published — there are no live facts to invalidate, and the consumer
    /// already fails closed on an absent snapshot.
    fn invalidate_portfolio_facts(&mut self, observed_at_ns: u64) -> Option<LossSnapshot> {
        self.state.invalidate_portfolio_facts();

        self.state.latest_snapshot.as_ref()?;

        let snapshot = LossSnapshot {
            source: LOSS_RUNTIME_FEED_SOURCE.to_string(),
            observed_at_ns,
            per_trade_pnl: None,
            daily_pnl: None,
            rolling_pnl: None,
            current_equity: None,
            peak_equity: None,
        };
        self.submit_admission.update_loss_snapshot(snapshot.clone());
        self.state.latest_snapshot = Some(snapshot.clone());
        Some(snapshot)
    }

    fn invoke_halt_action(&self, snapshot: &LossSnapshot) {
        if let Some(handler) = self.halt_action_handler.as_ref() {
            handler(Some(snapshot), snapshot.observed_at_ns);
        }
    }
}

fn invoke_loss_halt_action(
    handler: Option<LossGovernorHaltActionHandler>,
    snapshot: Option<&LossSnapshot>,
) {
    if let (Some(handler), Some(snapshot)) = (handler, snapshot) {
        handler(Some(snapshot), snapshot.observed_at_ns);
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
            position_pnls: HashMap::new(),
            portfolio_baseline_per_trade: None,
            current_equity: None,
            peak_equity: None,
            latest_snapshot: None,
        }
    }

    /// Worst-of-N per-trade loss across concurrent open positions.
    ///
    /// Selects the most negative (min) PnL across all tracked open positions so a
    /// benign event on one leg can never mask a larger loss on another; ties on
    /// value break to the oldest observation. When no positions are open, falls
    /// back to the zero per-trade baseline established by a portfolio snapshot.
    fn per_trade_pnl(&self) -> Option<TimedDecimal> {
        self.position_pnls
            .values()
            .copied()
            .min_by(|left, right| {
                left.value
                    .cmp(&right.value)
                    .then_with(|| left.observed_at_ns.cmp(&right.observed_at_ns))
            })
            .or(self.portfolio_baseline_per_trade)
    }

    /// Clear the cached portfolio facts on a corrupt heartbeat so the feed
    /// republishes from scratch on the next good snapshot. Open per-position
    /// facts and peak equity are preserved, mirroring the original governor.
    fn invalidate_portfolio_facts(&mut self) {
        self.previous_daily_pnl = None;
        self.previous_total_equity = None;
        self.daily_pnl = None;
        self.rolling_samples.clear();
        self.rolling_pnl = None;
        self.portfolio_baseline_per_trade = None;
        self.current_equity = None;
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
    ) -> PeakEquityAction {
        let mut peak_equity_action = PeakEquityAction::Preserve;
        if let Some(previous_daily_pnl) = self.previous_daily_pnl {
            let cumulative_delta = daily_pnl - previous_daily_pnl.value;
            let rolling_delta = match self.previous_total_equity {
                Some(previous_total_equity) => {
                    let equity_delta = current_equity - previous_total_equity.value;
                    if cumulative_delta != daily_pnl && equity_delta == daily_pnl {
                        peak_equity_action = PeakEquityAction::RebaselineToCurrent;
                        daily_pnl
                    } else {
                        if cumulative_delta.is_zero() && !equity_delta.is_zero() {
                            peak_equity_action = PeakEquityAction::RebaselineToCurrent;
                        }
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
        peak_equity_action
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PositionPnlFact {
    account_id: AccountId,
    position_id: PositionId,
    currency: Currency,
    kind: PositionPnlKind,
    observed_at_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PositionPnlKind {
    /// Absolute trade-level PnL for one open position (seeded at zero on open,
    /// updated on change). Drives the worst-of-N per-trade selection.
    Absolute(Decimal),
    /// The position is closed and must leave the worst-of-N set.
    Closed,
    /// A non-PnL adjustment (e.g. commission) that must not overwrite the
    /// worst-of-open selection.
    Adjustment,
}

fn position_pnl_fact(event: &PositionEvent) -> Option<PositionPnlFact> {
    match event {
        // Start tracking the position at zero so a later benign change cannot
        // resurrect a stale value, matching the original governor's open-at-zero.
        PositionEvent::PositionOpened(opened) => Some(PositionPnlFact {
            account_id: opened.account_id,
            position_id: opened.position_id,
            currency: opened.currency,
            kind: PositionPnlKind::Absolute(Decimal::ZERO),
            observed_at_ns: opened.ts_event.as_u64(),
        }),
        PositionEvent::PositionChanged(changed) => Some(PositionPnlFact {
            account_id: changed.account_id,
            position_id: changed.position_id,
            currency: changed.unrealized_pnl.currency,
            kind: PositionPnlKind::Absolute(combined_position_pnl(
                changed.realized_pnl,
                changed.unrealized_pnl,
            )?),
            observed_at_ns: changed.ts_event.as_u64(),
        }),
        // Close removes the leg regardless of PnL combinability; the per-trade
        // figure falls back to the next-worst open position or the baseline.
        PositionEvent::PositionClosed(closed) => Some(PositionPnlFact {
            account_id: closed.account_id,
            position_id: closed.position_id,
            currency: closed.unrealized_pnl.currency,
            kind: PositionPnlKind::Closed,
            observed_at_ns: closed.ts_event.as_u64(),
        }),
        PositionEvent::PositionAdjusted(adjusted) => {
            let pnl_change = adjusted.pnl_change?;
            Some(PositionPnlFact {
                account_id: adjusted.account_id,
                position_id: adjusted.position_id,
                currency: pnl_change.currency,
                kind: PositionPnlKind::Adjustment,
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
