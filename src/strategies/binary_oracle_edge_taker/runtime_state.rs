use std::collections::BTreeMap;

use nautilus_model::identifiers::InstrumentId;

use crate::{
    bolt_v3_book_sizing::{OutcomeBookState, OutcomeBookSubscriptions, OutcomePreparedBooks},
    bolt_v3_market_families::{MarketSelectionOutcome, SelectedMarketSourceIdentity},
    bolt_v3_numeric::{MILLIS_PER_SECOND_U64, is_positive_finite},
    bolt_v3_reference_price::ReferenceQuote,
    bolt_v3_trade_flow::SignedTradeFlow,
};

use super::{
    COUNTER_INCREMENT_U64, INITIAL_COUNTER_U64,
    selection::{CandidateMarket, RuntimeSelectionSnapshot, SelectionPhase, SelectionState},
};

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(super) enum VenueHealth {
    Healthy,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum VenueKind {
    Orderbook,
    Oracle,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(super) struct EffectiveVenueState {
    pub(super) venue_name: String,
    pub(super) base_weight: f64,
    pub(super) effective_weight: f64,
    pub(super) stale: bool,
    pub(super) health: VenueHealth,
    pub(super) observed_ts_ms: Option<u64>,
    pub(super) venue_kind: VenueKind,
    pub(super) observed_price: Option<f64>,
    pub(super) observed_bid: Option<f64>,
    pub(super) observed_ask: Option<f64>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ReferenceSnapshot {
    pub(super) ts_ms: u64,
    pub(super) topic: String,
    pub(super) fair_value: Option<f64>,
    pub(super) confidence: f64,
    pub(super) venues: Vec<EffectiveVenueState>,
}

impl OutcomePreparedBooks {
    fn from_market(market: &CandidateMarket) -> Self {
        Self {
            up: OutcomeBookState::from_instrument_id(InstrumentId::from(
                market.up.instrument_id.as_str(),
            )),
            down: OutcomeBookState::from_instrument_id(InstrumentId::from(
                market.down.instrument_id.as_str(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ActiveMarketState {
    pub(super) phase: SelectionPhase,
    pub(super) market_id: Option<String>,
    pub(super) source_identity: Option<SelectedMarketSourceIdentity>,
    pub(super) instrument_id: Option<InstrumentId>,
    pub(super) price_to_beat: Option<f64>,
    pub(super) market_selection_outcome: MarketSelectionOutcome,
    pub(super) interval_start_ms: Option<u64>,
    pub(super) interval_end_ms: Option<u64>,
    pub(super) selection_published_at_ms: Option<u64>,
    pub(super) seconds_to_expiry_at_selection: Option<u64>,
    pub(super) interval_open: Option<f64>,
    pub(super) reference_current_price: Option<f64>,
    pub(super) reference_current_price_source_id: Option<String>,
    pub(super) reference_current_price_failed_over: Option<bool>,
    pub(super) reference_current_price_ts_ms: Option<u64>,
    pub(super) last_reference_ts_ms: Option<u64>,
    pub(super) last_resolution_ts_ms: Option<u64>,
    /// Observable count of resolution-strike updates rejected because their
    /// `window_open_ms` did not match this market's interval-open while the
    /// market was configured (non-Idle). A configured mismatch is a fail-closed
    /// anomaly distinct from an Idle drop; this counter makes it observable.
    pub(super) resolution_strike_window_mismatch_count: u64,
    pub(super) warmup_count: u64,
    pub(super) warmup_target: u64,
    pub(super) books: OutcomePreparedBooks,
    pub(super) trade_flow: BTreeMap<InstrumentId, SignedTradeFlow>,
    pub(super) fast_venue_incoherent: bool,
    pub(super) forced_flat: bool,
}

impl OutcomeBookSubscriptions {
    pub(super) fn from_market(market: &CandidateMarket) -> Self {
        Self {
            up_instrument_id: Some(InstrumentId::from(market.up.instrument_id.as_str())),
            down_instrument_id: Some(InstrumentId::from(market.down.instrument_id.as_str())),
            tracked_position_instrument_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MarketLifecycleLedger {
    pub(super) cooldown_expires_at_ms: Option<u64>,
    pub(super) churn_count: u64,
}

impl MarketLifecycleLedger {
    pub(super) fn empty() -> Self {
        Self {
            cooldown_expires_at_ms: None,
            churn_count: INITIAL_COUNTER_U64,
        }
    }

    pub(super) fn in_cooldown(&self, now_ms: u64) -> bool {
        self.cooldown_expires_at_ms
            .is_some_and(|expiry_ms| now_ms < expiry_ms)
    }
}

impl ActiveMarketState {
    pub(super) fn idle() -> Self {
        Self {
            phase: SelectionPhase::Idle,
            market_id: None,
            source_identity: None,
            instrument_id: None,
            price_to_beat: None,
            market_selection_outcome: MarketSelectionOutcome::Current,
            interval_start_ms: None,
            interval_end_ms: None,
            selection_published_at_ms: None,
            seconds_to_expiry_at_selection: None,
            interval_open: None,
            reference_current_price: None,
            reference_current_price_source_id: None,
            reference_current_price_failed_over: None,
            reference_current_price_ts_ms: None,
            last_reference_ts_ms: None,
            last_resolution_ts_ms: None,
            resolution_strike_window_mismatch_count: INITIAL_COUNTER_U64,
            warmup_count: INITIAL_COUNTER_U64,
            warmup_target: INITIAL_COUNTER_U64,
            books: OutcomePreparedBooks::empty(),
            trade_flow: BTreeMap::new(),
            fast_venue_incoherent: false,
            forced_flat: false,
        }
    }

    pub(super) fn from_snapshot(snapshot: &RuntimeSelectionSnapshot, warmup_target: u64) -> Self {
        match &snapshot.decision.state {
            SelectionState::Active { market } => {
                Self::from_market(market, warmup_target, SelectionPhase::Active, false)
            }
            #[cfg(test)]
            SelectionState::Freeze { market, .. } => {
                Self::from_market(market, warmup_target, SelectionPhase::Freeze, true)
            }
            SelectionState::Idle { .. } => {
                let mut idle = Self::idle();
                idle.forced_flat = true;
                idle
            }
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
            source_identity: Some(market.source_identity.clone()),
            instrument_id: Some(InstrumentId::from(market.instrument_id.as_str())),
            price_to_beat: market.price_to_beat,
            market_selection_outcome: market.selection_outcome,
            interval_start_ms: Some(market.start_ts_ms),
            interval_end_ms: Some(market.expiration_ts_ms),
            selection_published_at_ms: None,
            seconds_to_expiry_at_selection: Some(market.seconds_to_end),
            interval_open: None,
            reference_current_price: None,
            reference_current_price_source_id: None,
            reference_current_price_failed_over: None,
            reference_current_price_ts_ms: None,
            last_reference_ts_ms: None,
            last_resolution_ts_ms: None,
            resolution_strike_window_mismatch_count: INITIAL_COUNTER_U64,
            warmup_count: INITIAL_COUNTER_U64,
            warmup_target,
            books: OutcomePreparedBooks::from_market(market),
            trade_flow: BTreeMap::new(),
            fast_venue_incoherent: false,
            forced_flat,
        }
    }

    pub(super) fn same_boundary(&self, other: &Self) -> bool {
        self.phase == other.phase
            && self.market_id == other.market_id
            && self.instrument_id == other.instrument_id
            && self.market_selection_outcome == other.market_selection_outcome
            && self.interval_start_ms == other.interval_start_ms
            && self.interval_end_ms == other.interval_end_ms
    }

    /// Binds the live resolution strike (Chainlink `IndexPriceUpdate`) to the
    /// market's interval-open boundary and sets it as the `price_to_beat`.
    ///
    /// Fail-closed: a strike whose `window_open_ms` does not equal this market's
    /// `interval_start_ms`, or a non-positive/non-finite value, or an idle/
    /// unbound state, is ignored and leaves `price_to_beat` unchanged. The entry
    /// gate stays blocked while `price_to_beat` is `None`.
    pub(super) fn observe_resolution_strike(
        &mut self,
        strike: f64,
        window_open_ms: u64,
        observed_ts_ms: u64,
    ) {
        if self.phase == SelectionPhase::Idle {
            return;
        }
        let Some(interval_start_ms) = self.interval_start_ms else {
            return;
        };
        if window_open_ms != interval_start_ms {
            // Configured (non-Idle, interval-bound) market whose strike report
            // disagrees with the selected interval-open. This is a fail-closed
            // anomaly — the strike feed is reporting for the wrong window — and
            // must be observable rather than a silent drop. Record it and warn;
            // `price_to_beat` is left untouched so entry stays fail-closed.
            self.resolution_strike_window_mismatch_count = self
                .resolution_strike_window_mismatch_count
                .saturating_add(1);
            log::warn!(
                "binary_oracle_edge_taker resolution-strike window mismatch (fail-closed): market_id={:?} window_open_ms={} interval_start_ms={} strike={} — strike rejected, price_to_beat unchanged",
                self.market_id,
                window_open_ms,
                interval_start_ms,
                strike,
            );
            return;
        }
        if !is_positive_finite(strike) {
            return;
        }
        self.price_to_beat = Some(strike);
        self.last_resolution_ts_ms = Some(observed_ts_ms);
    }

    pub(super) fn warmup_complete(&self) -> bool {
        self.warmup_target > INITIAL_COUNTER_U64 && self.warmup_count >= self.warmup_target
    }

    pub(super) fn apply_selection_timing(&mut self, snapshot: &RuntimeSelectionSnapshot) {
        match &snapshot.decision.state {
            SelectionState::Active { market } => {
                self.selection_published_at_ms = Some(snapshot.published_at_ms);
                self.market_selection_outcome = market.selection_outcome;
                self.interval_end_ms = Some(market.expiration_ts_ms);
                self.seconds_to_expiry_at_selection = Some(market.seconds_to_end);
            }
            #[cfg(test)]
            SelectionState::Freeze { market, .. } => {
                self.selection_published_at_ms = Some(snapshot.published_at_ms);
                self.market_selection_outcome = market.selection_outcome;
                self.interval_end_ms = Some(market.expiration_ts_ms);
                self.seconds_to_expiry_at_selection = Some(market.seconds_to_end);
            }
            SelectionState::Idle { .. } => {
                self.selection_published_at_ms = None;
                self.market_selection_outcome = MarketSelectionOutcome::Current;
                self.interval_end_ms = None;
                self.seconds_to_expiry_at_selection = None;
            }
        }
    }

    pub(super) fn seconds_to_expiry_at(&self, now_ms: u64) -> Option<u64> {
        let published_at_ms = self.selection_published_at_ms?;
        let seconds_to_expiry_at_selection = self.seconds_to_expiry_at_selection?;
        let elapsed_seconds = now_ms.saturating_sub(published_at_ms) / MILLIS_PER_SECOND_U64;
        Some(seconds_to_expiry_at_selection.saturating_sub(elapsed_seconds))
    }

    pub(super) fn observe_reference_price_quote(
        &mut self,
        quote: &ReferenceQuote,
        failed_over: bool,
    ) -> bool {
        if self.phase == SelectionPhase::Idle {
            return false;
        }
        let Some(interval_start_ms) = self.interval_start_ms else {
            return false;
        };
        if quote.observed_ts_ms() < interval_start_ms {
            return false;
        }
        let same_reference_source = self
            .reference_current_price_source_id
            .as_deref()
            .is_some_and(|source_id| source_id == quote.source_id());
        if same_reference_source
            && self
                .reference_current_price_ts_ms
                .is_some_and(|last_ts_ms| quote.observed_ts_ms() <= last_ts_ms)
        {
            return false;
        }

        self.reference_current_price = Some(quote.price());
        self.reference_current_price_source_id = Some(quote.source_id().to_string());
        self.reference_current_price_failed_over = Some(failed_over);
        self.reference_current_price_ts_ms = Some(quote.observed_ts_ms());
        self.last_reference_ts_ms = Some(quote.observed_ts_ms());
        if self.interval_open.is_none()
            && let Some(anchor_price) = self
                .price_to_beat
                .filter(|value| is_positive_finite(*value))
        {
            self.interval_open = Some(anchor_price);
        }
        if self.price_to_beat.is_some_and(is_positive_finite) {
            self.warmup_count = self.warmup_count.saturating_add(COUNTER_INCREMENT_U64);
        }
        true
    }

    pub(super) fn clear_reference_price_quote(&mut self) {
        self.reference_current_price = None;
        self.reference_current_price_source_id = None;
        self.reference_current_price_failed_over = None;
        self.reference_current_price_ts_ms = None;
    }

    pub(super) fn reset_reference_price_quote(&mut self) {
        self.clear_reference_price_quote();
        self.last_reference_ts_ms = None;
    }

    #[cfg(test)]
    pub(super) fn observe_reference_snapshot(&mut self, snapshot: &ReferenceSnapshot) {
        if self.phase == SelectionPhase::Idle {
            return;
        }
        let Some(interval_start_ms) = self.interval_start_ms else {
            return;
        };
        let Some(anchor_price) = self
            .price_to_beat
            .filter(|value| is_positive_finite(*value))
        else {
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
            self.interval_open = Some(anchor_price);
        }
        self.warmup_count += 1;
    }
}

pub(super) fn reference_current_price_boundary_changed(
    previous: &ActiveMarketState,
    current: &ActiveMarketState,
) -> bool {
    previous.market_id != current.market_id
        || previous.instrument_id != current.instrument_id
        || previous.interval_start_ms != current.interval_start_ms
        || previous.interval_end_ms != current.interval_end_ms
}
