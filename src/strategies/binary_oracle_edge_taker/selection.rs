//! Market-selection / candidate-snapshot cluster for the binary-oracle
//! edge-taker strategy.
//!
//! Pure relocation out of the parent `mod.rs` (slice A3): the market-family
//! selection types, the runtime-selection snapshot constructors, and the
//! execution-venue routing predicates live here so the parent module's runtime,
//! state, and entry-decision code reference them through `use
//! self::selection::{…}`. No behavior change — the bodies, the `#[cfg(test)]`
//! arms, and the test-only `SelectionState::Freeze` variant are verbatim.
//!
//! Visibility is `pub(super)` throughout: every item is consumed only by the
//! parent module tree (production code in `mod.rs` plus the parent
//! `#[cfg(test)] mod tests` block), never outside the strategy. No `pub use`
//! re-export is added.

use std::str::FromStr;

use nautilus_model::identifiers::{InstrumentId, Venue};
use nautilus_model::instruments::InstrumentAny;

use crate::bolt_v3_current_evidence::StrategyInputMarketSelectionOutcome;
use crate::bolt_v3_market_families::{
    self, MarketSelectionOutcome, MarketSelectionTarget, SelectedMarketEvidenceIdentity,
    SelectedMarketSourceIdentity,
};

use super::{ActiveMarketState, BinaryOracleEdgeTakerConfig, OutcomeBookSubscriptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SelectionPhase {
    Active,
    Freeze,
    Idle,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct CandidateOutcome {
    pub(super) instrument_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct CandidateMarket {
    pub(super) market_id: String,
    pub(super) instrument_id: String,
    pub(super) up: CandidateOutcome,
    pub(super) down: CandidateOutcome,
    pub(super) evidence_identity: SelectedMarketEvidenceIdentity,
    pub(super) source_identity: SelectedMarketSourceIdentity,
    pub(super) selection_outcome: MarketSelectionOutcome,
    pub(super) price_to_beat: Option<f64>,
    pub(super) start_ts_ms: u64,
    pub(super) expiration_ts_ms: u64,
    pub(super) seconds_to_end: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum SelectionState {
    // Boxed because the selected market now carries the market's complete
    // evidence identity, which makes this variant far larger than `Idle` and
    // this enum is cloned per selection pass.
    Active {
        market: Box<CandidateMarket>,
    },
    #[cfg(test)]
    Freeze {
        market: Box<CandidateMarket>,
        reason: String,
    },
    Idle {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct SelectionDecision {
    pub(super) ruleset_id: String,
    pub(super) state: SelectionState,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct RuntimeSelectionSnapshot {
    pub(super) ruleset_id: String,
    pub(super) decision: SelectionDecision,
    pub(super) eligible_candidates: Vec<CandidateMarket>,
    pub(super) published_at_ms: u64,
}

pub(super) const TARGET_MARKET_NOT_FOUND_REASON: &str = stringify!(target_market_not_found);

pub(super) fn apply_selection_snapshot_to_active(
    active: &mut ActiveMarketState,
    snapshot: &RuntimeSelectionSnapshot,
    warmup_target: u64,
) {
    let previous_books = active.books.clone();
    let previous_trade_flow = std::mem::take(&mut active.trade_flow);
    let next = ActiveMarketState::from_snapshot(snapshot, warmup_target);
    let preserve_books = active.market_id.is_some()
        && active.market_id == next.market_id
        && active.books.up.instrument_id == next.books.up.instrument_id
        && active.books.down.instrument_id == next.books.down.instrument_id;
    if active.same_boundary(&next) {
        active.trade_flow = previous_trade_flow;
        return;
    }
    if same_market_transition(active, &next) {
        active.phase = next.phase;
        active.forced_flat = next.forced_flat;
        active.market_selection_outcome = next.market_selection_outcome;
        active.interval_end_ms = next.interval_end_ms;
        active.trade_flow = previous_trade_flow;
        return;
    }
    *active = next;
    active.trade_flow = previous_trade_flow;
    if preserve_books {
        active.books = previous_books;
    }
}

pub(super) fn same_market_transition(
    current: &ActiveMarketState,
    next: &ActiveMarketState,
) -> bool {
    current.market_id.is_some()
        && current.market_id == next.market_id
        && current.evidence_identity == next.evidence_identity
        && current.instrument_id == next.instrument_id
        && current.market_selection_outcome == next.market_selection_outcome
        && current.interval_start_ms == next.interval_start_ms
        && current.interval_end_ms == next.interval_end_ms
}

pub(super) fn same_market_interval_rollover(
    current: &ActiveMarketState,
    next: &ActiveMarketState,
) -> bool {
    current.market_id.is_some()
        && current.market_id == next.market_id
        && current.instrument_id == next.instrument_id
        && current.interval_start_ms != next.interval_start_ms
}

pub(super) fn selection_book_subscriptions(
    snapshot: &RuntimeSelectionSnapshot,
) -> OutcomeBookSubscriptions {
    match &snapshot.decision.state {
        SelectionState::Active { market } => OutcomeBookSubscriptions::from_market(market),
        #[cfg(test)]
        SelectionState::Freeze { market, .. } => OutcomeBookSubscriptions::from_market(market),
        SelectionState::Idle { .. } => OutcomeBookSubscriptions::empty(),
    }
}

/// True unless `snapshot` selects an Active (or, in tests, Freeze) market whose up or down outcome
/// instrument is on a venue other than `execution_venue`. An Idle snapshot has no selected market to
/// route a real order to and trivially matches. An outcome instrument id that cannot be parsed fails
/// closed (treated as NOT on the execution venue), so a malformed selection can never pass the gate.
pub(super) fn selected_market_on_execution_venue(
    snapshot: &RuntimeSelectionSnapshot,
    execution_venue: Venue,
) -> bool {
    let market = match &snapshot.decision.state {
        SelectionState::Active { market } => market,
        #[cfg(test)]
        SelectionState::Freeze { market, .. } => market,
        SelectionState::Idle { .. } => return true,
    };
    outcome_on_execution_venue(&market.up, execution_venue)
        && outcome_on_execution_venue(&market.down, execution_venue)
}

pub(super) fn outcome_on_execution_venue(
    outcome: &CandidateOutcome,
    execution_venue: Venue,
) -> bool {
    InstrumentId::from_str(&outcome.instrument_id)
        .map(|instrument_id| instrument_id.venue == execution_venue)
        .unwrap_or(false)
}

pub(super) fn selection_snapshot_from_instruments(
    config: &BinaryOracleEdgeTakerConfig,
    instruments: &[InstrumentAny],
    now_ms: u64,
) -> RuntimeSelectionSnapshot {
    let Some(market) = select_configured_market_from_instruments(config, instruments, now_ms)
    else {
        return idle_selection_snapshot(config, now_ms, TARGET_MARKET_NOT_FOUND_REASON);
    };
    selection_snapshot_for_state(
        config,
        now_ms,
        SelectionState::Active {
            market: Box::new(market),
        },
    )
}

pub(super) fn idle_selection_snapshot(
    config: &BinaryOracleEdgeTakerConfig,
    now_ms: u64,
    reason: &str,
) -> RuntimeSelectionSnapshot {
    selection_snapshot_for_state(
        config,
        now_ms,
        SelectionState::Idle {
            reason: reason.to_string(),
        },
    )
}

pub(super) fn selection_snapshot_for_state(
    config: &BinaryOracleEdgeTakerConfig,
    now_ms: u64,
    state: SelectionState,
) -> RuntimeSelectionSnapshot {
    let ruleset_id = config.configured_target_id.clone();
    RuntimeSelectionSnapshot {
        ruleset_id: ruleset_id.clone(),
        decision: SelectionDecision { ruleset_id, state },
        eligible_candidates: Vec::new(),
        published_at_ms: now_ms,
    }
}

pub(super) fn select_configured_market_from_instruments(
    config: &BinaryOracleEdgeTakerConfig,
    instruments: &[InstrumentAny],
    now_ms: u64,
) -> Option<CandidateMarket> {
    let cadence_seconds = i64::try_from(config.cadence_seconds).ok()?;
    let target = MarketSelectionTarget {
        family_key: &config.rotating_market_family,
        underlying_asset: &config.underlying_asset,
        cadence_seconds,
        cadence_slug_token: &config.cadence_slug_token,
        static_condition_id: config.static_condition_id.as_deref(),
        static_yes_outcome: config.static_yes_outcome.as_deref(),
        static_no_outcome: config.static_no_outcome.as_deref(),
    };
    let market = bolt_v3_market_families::select_binary_option_market_from_target(
        target,
        instruments,
        now_ms,
    )?;
    Some(CandidateMarket {
        market_id: market.market_id,
        instrument_id: market.instrument_id.to_string(),
        up: CandidateOutcome {
            instrument_id: market.up_instrument_id.to_string(),
        },
        down: CandidateOutcome {
            instrument_id: market.down_instrument_id.to_string(),
        },
        evidence_identity: market.evidence_identity,
        source_identity: market.source_identity,
        selection_outcome: market.selection_outcome,
        price_to_beat: None,
        start_ts_ms: market.start_timestamp_milliseconds,
        expiration_ts_ms: market.expiration_timestamp_milliseconds,
        seconds_to_end: market.seconds_to_end,
    })
}

pub(super) fn strategy_input_market_selection_outcome(
    outcome: MarketSelectionOutcome,
) -> StrategyInputMarketSelectionOutcome {
    match outcome {
        MarketSelectionOutcome::Current => StrategyInputMarketSelectionOutcome::Current,
        MarketSelectionOutcome::Next => StrategyInputMarketSelectionOutcome::Next,
    }
}
