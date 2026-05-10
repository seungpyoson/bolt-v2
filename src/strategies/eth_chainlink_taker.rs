use std::{
    any::Any,
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, VecDeque},
    rc::Rc,
};

use anyhow::{Context, Result, anyhow};
use nautilus_common::{
    actor::{DataActor, registry::try_get_actor_unchecked},
    component::Component,
    msgbus::{self, ShareableMessageHandler},
};
use nautilus_core::{UUID4, UnixNanos};
#[cfg(not(test))]
use nautilus_model::enums::BookType;
use nautilus_model::enums::PositionSide;
use nautilus_model::{
    enums::{BookAction, OrderSide, OrderType, TimeInForce},
    identifiers::{ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId},
    instruments::{Instrument, InstrumentAny},
    types::{Price, Quantity},
};
use nautilus_system::trader::Trader;
use nautilus_trading::{Strategy, StrategyConfig, StrategyCore, nautilus_strategy};
use rust_decimal::prelude::ToPrimitive;
use serde::Deserialize;
use serde_json::json;
use toml::Value;

use crate::{
    bolt_v3_decision_events::{
        BoltV3EntryEvaluationFacts, BoltV3ExitEvaluationFacts, BoltV3MarketSelectionResultFacts,
        BoltV3OrderSubmissionFacts, BoltV3PreSubmitRejectionFacts, BoltV3RejectedOrderFacts,
        validate_bolt_v3_market_selection_failure_reason,
    },
    platform::{
        polymarket_catalog::polymarket_instrument_id,
        reference::ReferenceSnapshot,
        ruleset::{CandidateMarket, RuntimeSelectionSnapshot, SelectionState},
        runtime::runtime_selection_topic,
    },
    strategies::registry::{
        BoltV3MarketSelectionContext, BoxedStrategy, StrategyBuildContext, StrategyBuilder,
    },
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
            period_duration_secs: u64 => as_integer, "integer", "an integer", "missing_period_duration_secs";
            reentry_cooldown_secs: u64 => as_integer, "integer", "an integer", "missing_reentry_cooldown_secs";
            max_position_usdc: f64 => as_float_or_integer, "float", "a float", "missing_max_position_usdc";
            book_impact_cap_bps: u64 => as_integer, "integer", "an integer", "missing_book_impact_cap_bps";
            risk_lambda: f64 => as_float_or_integer, "float", "a float", "missing_risk_lambda";
            worst_case_ev_min_bps: i64 => as_integer, "integer", "an integer", "missing_worst_case_ev_min_bps";
            exit_hysteresis_bps: i64 => as_integer, "integer", "an integer", "missing_exit_hysteresis_bps";
            vol_window_secs: u64 => as_integer, "integer", "an integer", "missing_vol_window_secs";
            vol_gap_reset_secs: u64 => as_integer, "integer", "an integer", "missing_vol_gap_reset_secs";
            vol_min_observations: u64 => as_integer, "integer", "an integer", "missing_vol_min_observations";
            vol_bridge_valid_secs: u64 => as_integer, "integer", "an integer", "missing_vol_bridge_valid_secs";
            pricing_kurtosis: f64 => as_float_or_integer, "float", "a float", "missing_pricing_kurtosis";
            theta_decay_factor: f64 => as_float_or_integer, "float", "a float", "missing_theta_decay_factor";
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
struct OutcomeBookState {
    instrument_id: Option<InstrumentId>,
    last_observed_instrument_id: Option<InstrumentId>,
    bid_levels: BTreeMap<Price, f64>,
    ask_levels: BTreeMap<Price, f64>,
    best_bid: Option<f64>,
    best_ask: Option<f64>,
    liquidity_available: Option<f64>,
}

impl OutcomeBookState {
    fn from_instrument_id(instrument_id: InstrumentId) -> Self {
        Self {
            instrument_id: Some(instrument_id),
            ..Self::default()
        }
    }

    fn is_priced(&self) -> bool {
        self.best_bid.is_some() && self.best_ask.is_some()
    }

    fn metadata_matches_selection(&self) -> bool {
        self.last_observed_instrument_id.is_some()
            && self.last_observed_instrument_id == self.instrument_id
    }

    fn update_from_deltas(&mut self, deltas: &nautilus_model::data::OrderBookDeltas) {
        for delta in &deltas.deltas {
            let price = delta.order.price;
            let size = delta.order.size.as_f64();
            let levels = match delta.order.side {
                OrderSide::Buy => Some(&mut self.bid_levels),
                OrderSide::Sell => Some(&mut self.ask_levels),
                _ => None,
            };

            match delta.action {
                BookAction::Add | BookAction::Update => {
                    if let Some(levels) = levels {
                        if size > 0.0 && size.is_finite() {
                            levels.insert(price, size);
                        } else {
                            levels.remove(&price);
                        }
                    }
                }
                BookAction::Delete => {
                    if let Some(levels) = levels {
                        levels.remove(&price);
                    }
                }
                BookAction::Clear => {
                    self.bid_levels.clear();
                    self.ask_levels.clear();
                }
            }
        }

        self.last_observed_instrument_id = Some(deltas.instrument_id);
        self.best_bid = self
            .bid_levels
            .last_key_value()
            .map(|(price, _)| price.as_f64());
        self.best_ask = self
            .ask_levels
            .first_key_value()
            .map(|(price, _)| price.as_f64());
        self.liquidity_available = Some(
            self.bid_levels.values().copied().sum::<f64>()
                + self.ask_levels.values().copied().sum::<f64>(),
        );
    }

    fn max_buy_execution_within_vwap_slippage_bps(
        &self,
        slippage_bps: u64,
    ) -> Option<ImpactCappedExecution> {
        let best_ask = self
            .best_ask
            .filter(|value| value.is_finite() && *value > 0.0)?;
        let allowed_vwap = best_ask * (1.0 + slippage_bps as f64 / BPS_DENOMINATOR);
        max_execution_within_vwap_limit(
            self.ask_levels
                .iter()
                .map(|(price, size)| (price.as_f64(), *size))
                .collect(),
            allowed_vwap,
            true,
        )
    }
}

fn max_execution_within_vwap_limit(
    levels: Vec<(f64, f64)>,
    allowed_vwap: f64,
    is_buy: bool,
) -> Option<ImpactCappedExecution> {
    if !allowed_vwap.is_finite() || allowed_vwap <= 0.0 {
        return None;
    }

    let mut cumulative_qty = 0.0;
    let mut cumulative_notional = 0.0;

    for (price, size) in levels {
        if !price.is_finite() || price <= 0.0 || !size.is_finite() || size <= 0.0 {
            continue;
        }

        let next_qty = cumulative_qty + size;
        let next_notional = cumulative_notional + price * size;
        let next_vwap = next_notional / next_qty;
        let within_limit = if is_buy {
            next_vwap <= allowed_vwap
        } else {
            next_vwap >= allowed_vwap
        };
        if within_limit {
            cumulative_qty = next_qty;
            cumulative_notional = next_notional;
            continue;
        }

        let partial_qty = if is_buy {
            let denominator = price - allowed_vwap;
            if denominator <= 0.0 {
                size
            } else {
                ((allowed_vwap * cumulative_qty - cumulative_notional) / denominator)
                    .clamp(0.0, size)
            }
        } else {
            let denominator = allowed_vwap - price;
            if denominator <= 0.0 {
                size
            } else {
                ((cumulative_notional - allowed_vwap * cumulative_qty) / denominator)
                    .clamp(0.0, size)
            }
        };

        let total_qty = cumulative_qty + partial_qty;
        let total_notional = cumulative_notional + partial_qty * price;
        return (total_qty > 0.0).then_some(ImpactCappedExecution {
            quantity: total_qty,
            vwap_price: total_notional / total_qty,
        });
    }

    (cumulative_qty > 0.0).then_some(ImpactCappedExecution {
        quantity: cumulative_qty,
        vwap_price: cumulative_notional / cumulative_qty,
    })
}

#[derive(Debug, Clone, Default, PartialEq)]
struct OutcomePreparedBooks {
    up: OutcomeBookState,
    down: OutcomeBookState,
}

impl OutcomePreparedBooks {
    fn from_market(market: &CandidateMarket) -> Self {
        Self {
            up: OutcomeBookState::from_instrument_id(polymarket_instrument_id(
                &market.condition_id,
                &market.up_token_id,
            )),
            down: OutcomeBookState::from_instrument_id(polymarket_instrument_id(
                &market.condition_id,
                &market.down_token_id,
            )),
        }
    }

    fn is_priced(&self) -> bool {
        self.up.is_priced() && self.down.is_priced()
    }

    fn metadata_matches_selection(&self) -> bool {
        self.up.metadata_matches_selection() && self.down.metadata_matches_selection()
    }

    fn minimum_liquidity(&self) -> Option<f64> {
        Some(
            self.up
                .liquidity_available?
                .min(self.down.liquidity_available?),
        )
    }

    fn update_from_deltas(&mut self, deltas: &nautilus_model::data::OrderBookDeltas) -> bool {
        if self.up.instrument_id == Some(deltas.instrument_id) {
            self.up.update_from_deltas(deltas);
            true
        } else if self.down.instrument_id == Some(deltas.instrument_id) {
            self.down.update_from_deltas(deltas);
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
struct ActiveMarketState {
    phase: SelectionPhase,
    market_id: Option<String>,
    instrument_id: Option<InstrumentId>,
    outcome_fees: OutcomeFeeState,
    price_to_beat: Option<f64>,
    interval_start_ms: Option<u64>,
    selection_published_at_ms: Option<u64>,
    seconds_to_expiry_at_selection: Option<u64>,
    interval_open: Option<f64>,
    last_reference_ts_ms: Option<u64>,
    warmup_count: u64,
    warmup_target: u64,
    books: OutcomePreparedBooks,
    fast_venue_incoherent: bool,
    forced_flat: bool,
    decision_trace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
struct FastSpotObservation {
    venue_name: String,
    price: f64,
    observed_ts_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct VenueTimingState {
    last_observed_ts_ms: Option<u64>,
    last_interval_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
struct VolatilitySample {
    ts_ms: u64,
    price: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ImpactCappedExecution {
    quantity: f64,
    vwap_price: f64,
}

#[derive(Debug, Clone, PartialEq)]
struct RealizedVolEstimator {
    window_ms: u64,
    gap_reset_ms: u64,
    min_observations: u64,
    bridge_valid_ms: u64,
    active_venue_name: Option<String>,
    samples: VecDeque<VolatilitySample>,
    last_ready_vol: Option<f64>,
    last_ready_ts_ms: Option<u64>,
}

impl RealizedVolEstimator {
    fn from_config(config: &EthChainlinkTakerConfig) -> Self {
        Self {
            window_ms: config.vol_window_secs.saturating_mul(MILLIS_PER_SECOND_U64),
            gap_reset_ms: config
                .vol_gap_reset_secs
                .saturating_mul(MILLIS_PER_SECOND_U64),
            min_observations: config.vol_min_observations,
            bridge_valid_ms: config
                .vol_bridge_valid_secs
                .saturating_mul(MILLIS_PER_SECOND_U64),
            active_venue_name: None,
            samples: VecDeque::new(),
            last_ready_vol: None,
            last_ready_ts_ms: None,
        }
    }

    fn empty_like(&self) -> Self {
        Self {
            window_ms: self.window_ms,
            gap_reset_ms: self.gap_reset_ms,
            min_observations: self.min_observations,
            bridge_valid_ms: self.bridge_valid_ms,
            active_venue_name: None,
            samples: VecDeque::new(),
            last_ready_vol: None,
            last_ready_ts_ms: None,
        }
    }

    fn reset(&mut self) {
        self.active_venue_name = None;
        self.samples.clear();
        self.last_ready_vol = None;
        self.last_ready_ts_ms = None;
    }

    fn observe(&mut self, sample: &FastSpotObservation) -> Option<f64> {
        if !sample.price.is_finite() || sample.price <= 0.0 {
            return None;
        }

        if self.active_venue_name.as_deref() != Some(sample.venue_name.as_str()) {
            self.reset();
            self.active_venue_name = Some(sample.venue_name.clone());
        }

        if let Some(previous) = self.samples.back() {
            if sample.observed_ts_ms <= previous.ts_ms {
                return self.current_vol_at(sample.observed_ts_ms);
            }
            if sample.observed_ts_ms.saturating_sub(previous.ts_ms) > self.gap_reset_ms {
                self.reset();
                self.active_venue_name = Some(sample.venue_name.clone());
            }
        }

        self.samples.push_back(VolatilitySample {
            ts_ms: sample.observed_ts_ms,
            price: sample.price,
        });
        self.evict_old_samples(sample.observed_ts_ms);

        if let Some(vol) = self.compute_ready_vol() {
            self.last_ready_vol = Some(vol);
            self.last_ready_ts_ms = Some(sample.observed_ts_ms);
        }

        self.current_vol_at(sample.observed_ts_ms)
    }

    fn current_vol_at(&self, now_ms: u64) -> Option<f64> {
        let last_ready_ts_ms = self.last_ready_ts_ms?;
        if now_ms.saturating_sub(last_ready_ts_ms) <= self.bridge_valid_ms {
            self.last_ready_vol
        } else {
            None
        }
    }

    fn evict_old_samples(&mut self, now_ms: u64) {
        let cutoff_ms = now_ms.saturating_sub(self.window_ms);
        while self.samples.len() > 1
            && self
                .samples
                .front()
                .is_some_and(|sample| sample.ts_ms < cutoff_ms)
        {
            let _ = self.samples.pop_front();
        }
    }

    fn compute_ready_vol(&self) -> Option<f64> {
        let min_observations = self.min_observations.max(1) as usize;
        let mut observation_count = 0usize;
        let mut elapsed_ms = 0u64;
        let mut sum_squared_returns = 0.0;

        let mut iter = self.samples.iter();
        let mut previous = iter.next()?;
        for current in iter {
            let dt_ms = current.ts_ms.saturating_sub(previous.ts_ms);
            if dt_ms == 0 {
                previous = current;
                continue;
            }
            if !current.price.is_finite()
                || current.price <= 0.0
                || !previous.price.is_finite()
                || previous.price <= 0.0
            {
                return None;
            }

            let log_return = (current.price / previous.price).ln();
            if !log_return.is_finite() {
                return None;
            }

            sum_squared_returns += log_return.powi(2);
            elapsed_ms = elapsed_ms.saturating_add(dt_ms);
            observation_count += 1;
            previous = current;
        }

        if observation_count < min_observations || elapsed_ms == 0 {
            return None;
        }

        let elapsed_secs = elapsed_ms as f64 / MILLIS_PER_SECOND_F64;
        let annualized_variance = (sum_squared_returns / elapsed_secs) * SECONDS_PER_YEAR_F64;
        let vol = annualized_variance.sqrt();
        if vol.is_finite() && vol > 0.0 {
            Some(vol)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct PricingState {
    last_reference_fair_value: Option<f64>,
    fast_spot: Option<FastSpotObservation>,
    realized_vol: RealizedVolEstimator,
    realized_vol_source_venue: Option<String>,
    realized_vol_by_venue: BTreeMap<String, RealizedVolEstimator>,
    venue_timing: BTreeMap<String, VenueTimingState>,
    last_lead_gap_probability: Option<f64>,
    last_jitter_penalty_probability: Option<f64>,
    last_lead_agreement_corr: Option<f64>,
    last_fast_venue_age_ms: Option<u64>,
    last_fast_venue_jitter_ms: Option<u64>,
    fast_venue_incoherent: bool,
    lead_quality_policy_applied: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct OpenPositionState {
    market_id: Option<String>,
    instrument_id: InstrumentId,
    position_id: PositionId,
    outcome_side: Option<OutcomeSide>,
    outcome_fees: OutcomeFeeState,
    historical_entry_fee_bps: Option<f64>,
    entry_order_side: OrderSide,
    side: PositionSide,
    quantity: Quantity,
    avg_px_open: f64,
    interval_open: Option<f64>,
    selection_published_at_ms: Option<u64>,
    seconds_to_expiry_at_selection: Option<u64>,
    book: OutcomeBookState,
}

#[derive(Debug, Clone, PartialEq)]
struct PendingEntryState {
    client_order_id: ClientOrderId,
    market_id: Option<String>,
    instrument_id: InstrumentId,
    outcome_side: Option<OutcomeSide>,
    outcome_fees: OutcomeFeeState,
    historical_entry_fee_bps: Option<f64>,
    interval_open: Option<f64>,
    selection_published_at_ms: Option<u64>,
    seconds_to_expiry_at_selection: Option<u64>,
    book: OutcomeBookState,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct PositionMaterializationSpec {
    instrument_id: InstrumentId,
    position_id: PositionId,
    entry_order_side: OrderSide,
    side: PositionSide,
    quantity: Quantity,
    avg_px_open: f64,
}

#[derive(Debug, Clone, PartialEq)]
struct PendingExitState {
    client_order_id: ClientOrderId,
    market_id: Option<String>,
    position_id: Option<PositionId>,
    fill_received: bool,
    close_received: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedPositionOrigin {
    StrategyEntry,
    RecoveryBootstrap,
}

#[derive(Debug, Clone, PartialEq)]
struct ManagedPositionState {
    position: OpenPositionState,
    origin: ManagedPositionOrigin,
}

#[derive(Debug, Clone, PartialEq)]
struct ExitPendingState {
    position: Option<ManagedPositionState>,
    pending_exit: PendingExitState,
}

impl ExitPendingState {
    fn is_terminal(&self) -> bool {
        self.pending_exit.fill_received && self.pending_exit.close_received
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryReconcileReason {
    AwaitingPositionMaterialization,
    UnsupportedEntryFillSide {
        order_side: OrderSide,
    },
    InvalidObservedPosition {
        entry_order_side: OrderSide,
        side: PositionSide,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnsupportedObservedReason {
    BootstrappedUnsupportedContract,
    LiveUnsupportedContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlindRecoveryReason {
    CacheProbeFailed,
    MultipleOpenPositions {
        count: usize,
    },
    InvalidBootstrappedPosition {
        entry_order_side: OrderSide,
        side: PositionSide,
    },
    InvalidLivePosition {
        entry_order_side: OrderSide,
        side: PositionSide,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct UnsupportedObservedState {
    observed: OpenPositionState,
    reason: UnsupportedObservedReason,
}

#[derive(Debug, Clone, PartialEq)]
struct BlindRecoveryState {
    reason: BlindRecoveryReason,
}

#[derive(Debug, Clone, PartialEq)]
enum ExposureState {
    Flat,
    PendingEntry(PendingEntryState),
    EntryReconcilePending {
        pending: PendingEntryState,
        reason: EntryReconcileReason,
    },
    Managed(ManagedPositionState),
    ExitPending(ExitPendingState),
    UnsupportedObserved(UnsupportedObservedState),
    BlindRecovery(BlindRecoveryState),
}

impl ExposureState {
    fn pending_entry(&self) -> Option<&PendingEntryState> {
        match self {
            Self::PendingEntry(pending) | Self::EntryReconcilePending { pending, .. } => {
                Some(pending)
            }
            _ => None,
        }
    }

    fn pending_entry_mut(&mut self) -> Option<&mut PendingEntryState> {
        match self {
            Self::PendingEntry(pending) | Self::EntryReconcilePending { pending, .. } => {
                Some(pending)
            }
            _ => None,
        }
    }

    fn managed_position(&self) -> Option<&ManagedPositionState> {
        match self {
            Self::Managed(position) => Some(position),
            Self::ExitPending(exit) => exit.position.as_ref(),
            _ => None,
        }
    }

    fn observed_position(&self) -> Option<&OpenPositionState> {
        match self {
            Self::Managed(position) => Some(&position.position),
            Self::ExitPending(exit) => exit.position.as_ref().map(|position| &position.position),
            Self::UnsupportedObserved(observed) => Some(&observed.observed),
            _ => None,
        }
    }

    fn observed_position_mut(&mut self) -> Option<&mut OpenPositionState> {
        match self {
            Self::Managed(position) => Some(&mut position.position),
            Self::ExitPending(exit) => exit
                .position
                .as_mut()
                .map(|position| &mut position.position),
            Self::UnsupportedObserved(observed) => Some(&mut observed.observed),
            _ => None,
        }
    }

    fn exit_pending(&self) -> Option<&ExitPendingState> {
        match self {
            Self::ExitPending(exit) => Some(exit),
            _ => None,
        }
    }

    fn exit_pending_mut(&mut self) -> Option<&mut ExitPendingState> {
        match self {
            Self::ExitPending(exit) => Some(exit),
            _ => None,
        }
    }

    fn occupancy(&self) -> Option<ExposureOccupancy> {
        match self {
            Self::Flat => None,
            Self::PendingEntry(_) => Some(ExposureOccupancy::PendingEntry),
            Self::EntryReconcilePending { .. } => Some(ExposureOccupancy::EntryReconcilePending),
            Self::Managed(_) => Some(ExposureOccupancy::ManagedPosition),
            Self::ExitPending(_) => Some(ExposureOccupancy::ExitPending),
            Self::UnsupportedObserved(_) => Some(ExposureOccupancy::UnsupportedObserved),
            Self::BlindRecovery(_) => Some(ExposureOccupancy::BlindRecovery),
        }
    }

    #[cfg(test)]
    fn blocks_new_entries(&self) -> bool {
        !matches!(self, Self::Flat)
    }

    fn is_recovering(&self) -> bool {
        match self {
            Self::Managed(position) => position.origin == ManagedPositionOrigin::RecoveryBootstrap,
            Self::ExitPending(exit) => exit.position.as_ref().is_some_and(|position| {
                position.origin == ManagedPositionOrigin::RecoveryBootstrap
            }),
            Self::EntryReconcilePending { .. }
            | Self::UnsupportedObserved(_)
            | Self::BlindRecovery(_) => true,
            Self::Flat | Self::PendingEntry(_) => false,
        }
    }

    fn current_position_market_id(&self) -> Option<String> {
        self.managed_position()
            .and_then(|position| position.position.market_id.clone())
            .or_else(|| {
                self.exit_pending()
                    .and_then(|exit| exit.pending_exit.market_id.clone())
            })
    }
}

fn supports_strategy_managed_position(entry_order_side: OrderSide, side: PositionSide) -> bool {
    matches!(
        (entry_order_side, side),
        (OrderSide::Buy, PositionSide::Long)
    )
}

fn is_observed_open_side(side: PositionSide) -> bool {
    matches!(side, PositionSide::Long | PositionSide::Short)
}

fn infer_strategy_position_side_from_entry_fill(
    entry_order_side: OrderSide,
) -> Option<PositionSide> {
    match entry_order_side {
        OrderSide::Buy => Some(PositionSide::Long),
        _ => None,
    }
}

fn infer_strategy_outcome_side(
    entry_order_side: OrderSide,
    position_side: PositionSide,
    instrument_id: InstrumentId,
) -> Option<OutcomeSide> {
    if supports_strategy_managed_position(entry_order_side, position_side) {
        EthChainlinkTaker::infer_outcome_side_from_instrument_id(instrument_id)
    } else {
        None
    }
}

fn strategy_entry_order_side(_selected_side: OutcomeSide) -> OrderSide {
    OrderSide::Buy
}

fn managed_position_effective_entry_cost(position: &OpenPositionState) -> Option<f64> {
    match (position.entry_order_side, position.side) {
        (OrderSide::Buy, PositionSide::Long) => Some(position.avg_px_open),
        _ => None,
    }
    .filter(|effective_cost| effective_cost.is_finite() && *effective_cost > 0.0)
}

fn managed_position_exit_order(position: &OpenPositionState) -> Option<(OrderSide, f64)> {
    match position.side {
        PositionSide::Long => Some((OrderSide::Sell, position.book.best_bid?)),
        _ => None,
    }
    .filter(|(_, price)| price.is_finite() && *price > 0.0)
}

fn managed_position_exit_value(position: &OpenPositionState) -> Option<f64> {
    match position.side {
        PositionSide::Long => position.book.best_bid,
        _ => None,
    }
    .filter(|value| value.is_finite() && *value > 0.0)
}

impl PricingState {
    fn from_config(config: &EthChainlinkTakerConfig) -> Self {
        Self {
            last_reference_fair_value: None,
            fast_spot: None,
            realized_vol: RealizedVolEstimator::from_config(config),
            realized_vol_source_venue: None,
            realized_vol_by_venue: BTreeMap::new(),
            venue_timing: BTreeMap::new(),
            last_lead_gap_probability: None,
            last_jitter_penalty_probability: None,
            last_lead_agreement_corr: None,
            last_fast_venue_age_ms: None,
            last_fast_venue_jitter_ms: None,
            fast_venue_incoherent: false,
            lead_quality_policy_applied: false,
        }
    }

    fn observe_reference_snapshot(
        &mut self,
        snapshot: &ReferenceSnapshot,
        min_agreement_corr: f64,
        max_jitter_ms: u64,
    ) {
        if let Some(fair_value) = snapshot
            .fair_value
            .filter(|fair_value| fair_value.is_finite() && *fair_value > 0.0)
        {
            self.last_reference_fair_value = Some(fair_value);
        }

        let candidates = self.build_lead_venue_signals(snapshot);
        self.observe_realized_vol_candidates(&candidates, min_agreement_corr, max_jitter_ms);
        self.lead_quality_policy_applied = true;
        if let Some(candidate) =
            arbitrate_lead_reference(&candidates, min_agreement_corr, max_jitter_ms)
        {
            let fast_spot = FastSpotObservation {
                venue_name: candidate.venue_name.clone(),
                price: candidate
                    .price
                    .expect("selected lead venue should carry price"),
                observed_ts_ms: candidate
                    .observed_ts_ms
                    .expect("selected lead venue should carry timestamp"),
            };
            self.realized_vol = self.selected_realized_vol_for_candidate(candidate);
            self.realized_vol_source_venue = Some(candidate.venue_name.clone());
            self.fast_spot = Some(fast_spot);
            self.last_lead_gap_probability = Some(candidate.lead_gap_probability);
            self.last_jitter_penalty_probability = Some(if max_jitter_ms == 0 {
                0.0
            } else {
                (candidate.jitter_ms as f64 / max_jitter_ms as f64).clamp(0.0, 1.0)
            });
            self.last_lead_agreement_corr = Some(candidate.agreement_corr);
            self.last_fast_venue_age_ms = Some(candidate.age_ms);
            self.last_fast_venue_jitter_ms = Some(candidate.jitter_ms);
            self.fast_venue_incoherent = false;
        } else {
            self.fast_spot = None;
            self.last_lead_gap_probability = None;
            self.last_jitter_penalty_probability = None;
            self.last_lead_agreement_corr = None;
            self.last_fast_venue_age_ms = None;
            self.last_fast_venue_jitter_ms = None;
            self.fast_venue_incoherent = !candidates.is_empty();
        }
    }

    fn observe_realized_vol_candidates(
        &mut self,
        candidates: &[LeadVenueSignal],
        min_agreement_corr: f64,
        max_jitter_ms: u64,
    ) {
        let estimator_template = self.realized_vol.empty_like();

        for candidate in candidates {
            if !candidate.is_eligible(min_agreement_corr, max_jitter_ms) {
                continue;
            }
            let (Some(price), Some(observed_ts_ms)) = (candidate.price, candidate.observed_ts_ms)
            else {
                continue;
            };

            let estimator = self
                .realized_vol_by_venue
                .entry(candidate.venue_name.clone())
                .or_insert_with(|| estimator_template.clone());
            let _ = estimator.observe(&FastSpotObservation {
                venue_name: candidate.venue_name.clone(),
                price,
                observed_ts_ms,
            });
        }
    }

    fn selected_realized_vol_for_candidate(
        &self,
        candidate: &LeadVenueSignal,
    ) -> RealizedVolEstimator {
        self.realized_vol_by_venue
            .get(&candidate.venue_name)
            .cloned()
            .unwrap_or_else(|| {
                log::error!(
                    "eth_chainlink_taker selected lead venue missing realized-vol state: venue={}",
                    candidate.venue_name
                );
                self.realized_vol.empty_like()
            })
    }

    fn spot_price(&self) -> Option<f64> {
        self.fast_spot
            .as_ref()
            .map(|spot| spot.price)
            .or(self.last_reference_fair_value)
    }

    fn current_realized_vol_source_at(&self, now_ms: u64) -> (Option<String>, Option<u64>) {
        if self.realized_vol.current_vol_at(now_ms).is_none() {
            return (None, None);
        }

        (
            self.realized_vol_source_venue
                .clone()
                .or_else(|| self.fast_spot.as_ref().map(|spot| spot.venue_name.clone()))
                .or_else(|| self.realized_vol.active_venue_name.clone()),
            self.realized_vol.last_ready_ts_ms,
        )
    }

    fn build_lead_venue_signals(&mut self, snapshot: &ReferenceSnapshot) -> Vec<LeadVenueSignal> {
        let agreement_anchor = best_healthy_oracle_price(snapshot).or(snapshot.fair_value);
        let reference_anchor = snapshot.fair_value;

        snapshot
            .venues
            .iter()
            .filter_map(|venue| {
                if venue.venue_kind != crate::platform::reference::VenueKind::Orderbook
                    || venue.stale
                    || !matches!(
                        venue.health,
                        crate::platform::reference::VenueHealth::Healthy
                    )
                    || !venue.effective_weight.is_finite()
                    || venue.effective_weight <= 0.0
                {
                    return None;
                }

                let observed_price = venue.observed_price?;
                let observed_ts_ms = venue.observed_ts_ms?;
                if !observed_price.is_finite() || observed_price <= 0.0 {
                    return None;
                }

                let timing = self
                    .venue_timing
                    .entry(venue.venue_name.clone())
                    .or_default();
                let age_ms = snapshot.ts_ms.saturating_sub(observed_ts_ms);
                let current_interval_ms = timing
                    .last_observed_ts_ms
                    .map(|last_ts_ms| observed_ts_ms.saturating_sub(last_ts_ms));
                let jitter_ms = match (current_interval_ms, timing.last_interval_ms) {
                    (Some(current_interval_ms), Some(last_interval_ms)) => {
                        current_interval_ms.abs_diff(last_interval_ms)
                    }
                    _ => 0,
                };
                timing.last_observed_ts_ms = Some(observed_ts_ms);
                timing.last_interval_ms = current_interval_ms;

                let agreement_corr = agreement_anchor
                    .filter(|anchor| anchor.is_finite() && *anchor > 0.0)
                    .map(|anchor| {
                        (1.0 - ((observed_price - anchor).abs() / anchor)).clamp(0.0, 1.0)
                    })
                    .unwrap_or(0.0);
                let lead_gap_probability = reference_anchor
                    .filter(|anchor| anchor.is_finite() && *anchor > 0.0)
                    .map(|anchor| ((observed_price - anchor).abs() / anchor).clamp(0.0, 1.0))
                    .unwrap_or(0.0);

                Some(LeadVenueSignal {
                    venue_name: venue.venue_name.clone(),
                    price: Some(observed_price),
                    observed_ts_ms: Some(observed_ts_ms),
                    age_ms,
                    jitter_ms,
                    agreement_corr,
                    effective_weight: venue.effective_weight,
                    lead_gap_probability,
                })
            })
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct OutcomeBookSubscriptions {
    up_instrument_id: Option<InstrumentId>,
    down_instrument_id: Option<InstrumentId>,
    tracked_position_instrument_id: Option<InstrumentId>,
}

impl OutcomeBookSubscriptions {
    fn from_market(market: &CandidateMarket) -> Self {
        Self {
            up_instrument_id: Some(polymarket_instrument_id(
                &market.condition_id,
                &market.up_token_id,
            )),
            down_instrument_id: Some(polymarket_instrument_id(
                &market.condition_id,
                &market.down_token_id,
            )),
            tracked_position_instrument_id: None,
        }
    }

    fn is_same_market(&self, other: &Self) -> bool {
        self.up_instrument_id == other.up_instrument_id
            && self.down_instrument_id == other.down_instrument_id
            && self.tracked_position_instrument_id == other.tracked_position_instrument_id
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

    fn market_ready(&self) -> bool {
        self.up_ready && self.down_ready
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct MarketLifecycleLedger {
    cooldown_expires_at_ms: Option<u64>,
    churn_count: u64,
}

impl MarketLifecycleLedger {
    fn in_cooldown(&self, now_ms: u64) -> bool {
        self.cooldown_expires_at_ms
            .is_some_and(|expiry_ms| now_ms < expiry_ms)
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
            price_to_beat: market.price_to_beat,
            interval_start_ms: Some(market.start_ts_ms),
            selection_published_at_ms: None,
            seconds_to_expiry_at_selection: Some(market.seconds_to_end),
            warmup_target,
            books: OutcomePreparedBooks::from_market(market),
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

    fn apply_selection_timing(&mut self, snapshot: &RuntimeSelectionSnapshot) {
        match &snapshot.decision.state {
            SelectionState::Active { market } | SelectionState::Freeze { market, .. } => {
                self.selection_published_at_ms = Some(snapshot.published_at_ms);
                self.seconds_to_expiry_at_selection = Some(market.seconds_to_end);
            }
            SelectionState::Idle { .. } => {
                self.selection_published_at_ms = None;
                self.seconds_to_expiry_at_selection = None;
            }
        }
    }

    fn seconds_to_expiry_at(&self, now_ms: u64) -> Option<u64> {
        let published_at_ms = self.selection_published_at_ms?;
        let seconds_to_expiry_at_selection = self.seconds_to_expiry_at_selection?;
        let elapsed_seconds = now_ms.saturating_sub(published_at_ms) / MILLIS_PER_SECOND_U64;
        Some(seconds_to_expiry_at_selection.saturating_sub(elapsed_seconds))
    }

    fn observe_reference_snapshot(&mut self, snapshot: &ReferenceSnapshot) {
        if self.phase == SelectionPhase::Idle {
            return;
        }
        let Some(interval_start_ms) = self.interval_start_ms else {
            return;
        };
        let Some(anchor_price) = self
            .price_to_beat
            .or_else(|| best_healthy_oracle_price(snapshot))
            .or(snapshot.fair_value)
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

pub struct EthChainlinkTaker {
    core: StrategyCore,
    config: EthChainlinkTakerConfig,
    context: StrategyBuildContext,
    active: ActiveMarketState,
    book_subscriptions: OutcomeBookSubscriptions,
    market_lifecycle: BTreeMap<String, MarketLifecycleLedger>,
    exposure: ExposureState,
    last_reported_exposure_occupancy: Cell<Option<ExposureOccupancy>>,
    pricing: PricingState,
    selection_handler: Option<ShareableMessageHandler>,
    reference_handler: Option<ShareableMessageHandler>,
    #[cfg(test)]
    book_subscription_events: Vec<BookSubscriptionEvent>,
}

impl EthChainlinkTaker {
    fn new(config: EthChainlinkTakerConfig, context: StrategyBuildContext) -> Self {
        let pricing = PricingState::from_config(&config);
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(StrategyId::from(config.strategy_id.as_str())),
                ..Default::default()
            }),
            config,
            context,
            active: ActiveMarketState::default(),
            book_subscriptions: OutcomeBookSubscriptions::default(),
            market_lifecycle: BTreeMap::new(),
            exposure: ExposureState::Flat,
            last_reported_exposure_occupancy: Cell::new(None),
            pricing,
            selection_handler: None,
            reference_handler: None,
            #[cfg(test)]
            book_subscription_events: Vec::new(),
        }
    }

    fn apply_selection_snapshot(&mut self, snapshot: RuntimeSelectionSnapshot) {
        let now_ms = snapshot.published_at_ms;
        let previous_active = self.active.clone();
        let previous_phase = previous_active.phase;
        let previous_fee_tokens = previous_active.outcome_fees.token_ids();
        let next_selection_books = selection_book_subscriptions(&snapshot);
        apply_selection_snapshot_to_active(
            &mut self.active,
            &snapshot,
            self.config.warmup_tick_count,
        );
        self.active.books.up.instrument_id = next_selection_books.up_instrument_id;
        self.active.books.down.instrument_id = next_selection_books.down_instrument_id;
        self.active.apply_selection_timing(&snapshot);
        let reactivated_into_active =
            previous_phase != SelectionPhase::Active && self.active.phase == SelectionPhase::Active;
        let same_market_interval_rollover =
            same_market_interval_rollover(&previous_active, &self.active);
        let next_fee_tokens = self.active.outcome_fees.token_ids();
        if previous_fee_tokens != next_fee_tokens
            || (same_market_interval_rollover && !next_fee_tokens.is_empty())
            || (reactivated_into_active && !next_fee_tokens.is_empty())
        {
            self.trigger_fee_warm_for_market();
            self.refresh_fee_readiness();
        }
        self.sync_exposure_context_from_active();
        self.prune_market_lifecycle(now_ms);
        self.refresh_book_subscriptions_for_current_state();
        if let Err(error) = self.write_bolt_v3_market_selection_result(&snapshot) {
            log::error!(
                "eth_chainlink_taker market selection evidence failed: strategy_id={} now_ms={} error={:#}",
                self.config.strategy_id,
                now_ms,
                error
            );
        }
        if self.exposure.managed_position().is_some()
            && let Err(error) = self.try_submit_exit_order(now_ms)
        {
            log::error!(
                "eth_chainlink_taker exit submit failed on selection update: strategy_id={} market_id={:?} now_ms={} error={:#}",
                self.config.strategy_id,
                self.active.market_id,
                now_ms,
                error,
            );
        }
    }

    fn observe_reference_snapshot(&mut self, snapshot: &ReferenceSnapshot) {
        self.active.observe_reference_snapshot(snapshot);
        self.pricing.observe_reference_snapshot(
            snapshot,
            self.config.lead_agreement_min_corr,
            self.config.lead_jitter_max_ms,
        );
        self.active.fast_venue_incoherent = self.pricing.fast_venue_incoherent;
        self.refresh_fee_readiness();
        self.sync_exposure_context_from_active();
        if self.exposure.managed_position().is_some()
            && let Err(error) = self.try_submit_exit_order(snapshot.ts_ms)
        {
            log::error!(
                "eth_chainlink_taker exit submit failed on reference update: strategy_id={} market_id={:?} ts_ms={} error={:#}",
                self.config.strategy_id,
                self.active.market_id,
                snapshot.ts_ms,
                error,
            );
        }
    }

    fn refresh_fee_readiness(&mut self) {
        refresh_fee_readiness_for_active(&mut self.active, self.context.fee_provider.as_ref());
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
        let selection_topic =
            runtime_selection_topic(&StrategyId::from(self.config.strategy_id.as_str()));
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
        let selection_topic =
            runtime_selection_topic(&StrategyId::from(self.config.strategy_id.as_str()));
        if let Some(handler) = self.selection_handler.take() {
            msgbus::unsubscribe_any(selection_topic.into(), &handler);
        }
        if let Some(handler) = self.reference_handler.take() {
            msgbus::unsubscribe_any(
                self.context.reference_publish_topic.clone().into(),
                &handler,
            );
        }
        self.replace_book_subscriptions(OutcomeBookSubscriptions::default());
    }

    fn replace_book_subscriptions(&mut self, next: OutcomeBookSubscriptions) {
        let current = self.book_subscriptions.clone();
        unsubscribe_missing_books(self, &current, &next);
        subscribe_new_books(self, &current, &next);
        self.book_subscriptions = next;
    }

    fn current_market_id(&self) -> Option<&str> {
        self.active.market_id.as_deref()
    }

    fn active_decision_trace_id(&mut self) -> String {
        self.active
            .decision_trace_id
            .get_or_insert_with(|| UUID4::new().to_string())
            .clone()
    }

    fn tracked_observed_position(&self) -> Option<&OpenPositionState> {
        self.exposure.observed_position()
    }

    fn tracked_observed_position_mut(&mut self) -> Option<&mut OpenPositionState> {
        self.exposure.observed_position_mut()
    }

    fn managed_position(&self) -> Option<&ManagedPositionState> {
        self.exposure.managed_position()
    }

    fn pending_entry(&self) -> Option<&PendingEntryState> {
        self.exposure.pending_entry()
    }

    fn pending_entry_mut(&mut self) -> Option<&mut PendingEntryState> {
        self.exposure.pending_entry_mut()
    }

    fn set_unsupported_observed_exposure(
        &mut self,
        observed: OpenPositionState,
        reason: UnsupportedObservedReason,
    ) {
        self.exposure =
            ExposureState::UnsupportedObserved(UnsupportedObservedState { observed, reason });
        self.refresh_book_subscriptions_for_current_state();
    }

    fn bootstrap_recovery_from_cache(&mut self) {
        let strategy_id = StrategyId::from(self.config.strategy_id.as_str());
        let cached_positions = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let cache = self.cache();
            cache
                .positions_open(None, None, Some(&strategy_id), None, None)
                .into_iter()
                .map(|position| OpenPositionState {
                    market_id: None,
                    instrument_id: position.instrument_id,
                    position_id: position.id,
                    outcome_side: infer_strategy_outcome_side(
                        position.entry,
                        position.side,
                        position.instrument_id,
                    ),
                    outcome_fees: OutcomeFeeState::default(),
                    historical_entry_fee_bps: None,
                    entry_order_side: position.entry,
                    side: position.side,
                    quantity: position.quantity,
                    avg_px_open: position.avg_px_open,
                    interval_open: None,
                    selection_published_at_ms: None,
                    seconds_to_expiry_at_selection: None,
                    book: OutcomeBookState::from_instrument_id(position.instrument_id),
                })
                .collect::<Vec<_>>()
        }));

        let cached_positions = match cached_positions {
            Ok(cached_positions) => cached_positions,
            Err(_) => {
                self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                    reason: BlindRecoveryReason::CacheProbeFailed,
                });
                log::warn!(
                    "eth_chainlink_taker recovery probe could not access cache: strategy_id={} entering fail-closed recovery mode",
                    self.config.strategy_id
                );
                return;
            }
        };

        if cached_positions.is_empty() {
            self.exposure = ExposureState::Flat;
            return;
        }

        if cached_positions.len() > 1 {
            self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::MultipleOpenPositions {
                    count: cached_positions.len(),
                },
            });
            log::error!(
                "eth_chainlink_taker recovery bootstrap found multiple open positions: strategy_id={} position_count={} leaving recovery mode blind to position bootstrap",
                self.config.strategy_id,
                cached_positions.len(),
            );
            return;
        }

        let open_position = cached_positions
            .into_iter()
            .next()
            .expect("checked non-empty recovery position set");
        if supports_strategy_managed_position(open_position.entry_order_side, open_position.side) {
            self.exposure = ExposureState::Managed(ManagedPositionState {
                position: open_position.clone(),
                origin: ManagedPositionOrigin::RecoveryBootstrap,
            });
            log::warn!(
                "eth_chainlink_taker recovery bootstrap loaded cached open position: strategy_id={} position_id={} instrument_id={} entry_order_side={:?} side={:?} quantity={} avg_px_open={}",
                self.config.strategy_id,
                open_position.position_id,
                open_position.instrument_id,
                open_position.entry_order_side,
                open_position.side,
                open_position.quantity,
                open_position.avg_px_open,
            );
        } else if is_observed_open_side(open_position.side) {
            self.exposure = ExposureState::UnsupportedObserved(UnsupportedObservedState {
                observed: open_position.clone(),
                reason: UnsupportedObservedReason::BootstrappedUnsupportedContract,
            });
            log::error!(
                "eth_chainlink_taker recovery bootstrap quarantined unsupported cached position: strategy_id={} position_id={} instrument_id={} entry_order_side={:?} side={:?} quantity={} avg_px_open={}",
                self.config.strategy_id,
                open_position.position_id,
                open_position.instrument_id,
                open_position.entry_order_side,
                open_position.side,
                open_position.quantity,
                open_position.avg_px_open,
            );
        } else {
            self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::InvalidBootstrappedPosition {
                    entry_order_side: open_position.entry_order_side,
                    side: open_position.side,
                },
            });
            log::error!(
                "eth_chainlink_taker recovery bootstrap received invalid cached position side: strategy_id={} position_id={} instrument_id={} entry_order_side={:?} side={:?}",
                self.config.strategy_id,
                open_position.position_id,
                open_position.instrument_id,
                open_position.entry_order_side,
                open_position.side,
            );
        }
    }

    fn exposure_occupancy(&self) -> Option<ExposureOccupancy> {
        self.exposure.occupancy()
    }

    fn clear_pending_entry_state(&mut self) {
        if matches!(self.exposure, ExposureState::PendingEntry(_)) {
            self.exposure = ExposureState::Flat;
            let now_ms = self.clock().timestamp_ns().as_u64() / 1_000_000;
            self.prune_market_lifecycle(now_ms);
        }
    }

    fn enforce_one_position_invariant(&self) -> Result<()> {
        let Some(occupancy) = self.exposure_occupancy() else {
            return Ok(());
        };

        let message = format!("one-position invariant occupied by {occupancy:?}");
        if cfg!(debug_assertions) {
            panic!("{message}");
        }

        self.report_one_position_invariant_violation(occupancy);
        anyhow::bail!("{message}");
    }

    fn report_one_position_invariant_violation(&self, occupancy: ExposureOccupancy) {
        if self.last_reported_exposure_occupancy.get() == Some(occupancy) {
            return;
        }
        self.last_reported_exposure_occupancy.set(Some(occupancy));
        let message = format!("one-position invariant occupied by {occupancy:?}");
        log::error!("{message}");
    }

    fn market_in_cooldown(&self, market_id: &str, now_ms: u64) -> bool {
        self.market_lifecycle
            .get(market_id)
            .is_some_and(|ledger| ledger.in_cooldown(now_ms))
    }

    fn arm_market_cooldown(&mut self, market_id: &str, now_ms: u64) {
        self.market_lifecycle
            .entry(market_id.to_string())
            .or_default()
            .cooldown_expires_at_ms = Some(
            now_ms.saturating_add(
                self.config
                    .reentry_cooldown_secs
                    .saturating_mul(MILLIS_PER_SECOND_U64),
            ),
        );
    }

    fn record_market_fill(&mut self, market_id: &str, now_ms: u64) {
        self.arm_market_cooldown(market_id, now_ms);
        let ledger = self
            .market_lifecycle
            .entry(market_id.to_string())
            .or_default();
        ledger.churn_count = ledger.churn_count.saturating_add(1);
        self.prune_market_lifecycle(now_ms);
    }

    #[cfg(test)]
    fn market_churn_count(&self, market_id: &str) -> u64 {
        self.market_lifecycle
            .get(market_id)
            .map(|ledger| ledger.churn_count)
            .unwrap_or(0)
    }

    fn prune_market_lifecycle(&mut self, now_ms: u64) {
        let retained_market_ids = self.retained_market_lifecycle_ids();
        self.market_lifecycle.retain(|market_id, ledger| {
            retained_market_ids.contains(market_id) || ledger.in_cooldown(now_ms)
        });
    }

    fn retained_market_lifecycle_ids(&self) -> BTreeSet<String> {
        let mut retained = BTreeSet::new();
        if let Some(market_id) = self.active.market_id.clone() {
            retained.insert(market_id);
        }
        if let Some(market_id) = self
            .pending_entry()
            .and_then(|pending| pending.market_id.clone())
        {
            retained.insert(market_id);
        }
        if let Some(market_id) = self
            .tracked_observed_position()
            .and_then(|position| position.market_id.clone())
        {
            retained.insert(market_id);
        }
        if let Some(market_id) = self
            .exposure
            .exit_pending()
            .and_then(|exit| exit.pending_exit.market_id.clone())
        {
            retained.insert(market_id);
        }
        retained
    }

    fn entry_gate_decision_at(&self, now_ms: u64) -> EntryGateDecision {
        let mut blocked_by = Vec::new();

        if self.active.phase != SelectionPhase::Active {
            blocked_by.push(EntryBlockReason::PhaseNotActive);
        }
        if !self.active.books.metadata_matches_selection() {
            blocked_by.push(EntryBlockReason::MetadataMismatch);
        }
        if !self.active.books.is_priced() {
            blocked_by.push(EntryBlockReason::ActiveBookNotPriced);
        }
        if self.active.interval_open.is_none() {
            blocked_by.push(EntryBlockReason::IntervalOpenMissing);
        }
        if !self.active.warmup_complete() {
            blocked_by.push(EntryBlockReason::WarmupIncomplete);
        }
        if !self.active.outcome_fees.market_ready() {
            blocked_by.push(EntryBlockReason::FeesNotReady);
        }
        if self.exposure.is_recovering() {
            blocked_by.push(EntryBlockReason::RecoveryMode);
        }
        if self
            .current_market_id()
            .is_some_and(|market_id| self.market_in_cooldown(market_id, now_ms))
        {
            blocked_by.push(EntryBlockReason::MarketCoolingDown);
        }
        for reason in self
            .active_forced_flat_reasons_at(now_ms)
            .into_iter()
            .filter(|reason| *reason != ForcedFlatReason::MetadataMismatch)
        {
            blocked_by.push(EntryBlockReason::ForcedFlat(reason));
        }
        if let Some(occupancy) = self.exposure_occupancy() {
            if should_report_one_position_gate_violation(occupancy) {
                self.report_one_position_invariant_violation(occupancy);
            }
            blocked_by.push(EntryBlockReason::OnePositionInvariant(occupancy));
        } else {
            self.last_reported_exposure_occupancy.set(None);
        }

        EntryGateDecision { blocked_by }
    }

    fn active_forced_flat_reasons_at(&self, now_ms: u64) -> Vec<ForcedFlatReason> {
        evaluate_forced_flat_predicates(&ForcedFlatInputs {
            phase: self.active.phase,
            metadata_matches_selection: self.active.books.metadata_matches_selection(),
            last_chainlink_ts_ms: self.active.last_reference_ts_ms,
            now_ms,
            stale_chainlink_after_ms: self.config.forced_flat_stale_chainlink_ms,
            liquidity_available: self.active.books.minimum_liquidity(),
            min_liquidity_required: self.config.forced_flat_thin_book_min_liquidity,
            fast_venue_incoherent: self.active.fast_venue_incoherent,
        })
        .into_iter()
        .collect()
    }

    fn position_forced_flat_reasons_at(&self, now_ms: u64) -> Vec<ForcedFlatReason> {
        let Some(open_position) = self.managed_position().map(|managed| &managed.position) else {
            return self.active_forced_flat_reasons_at(now_ms);
        };

        evaluate_forced_flat_predicates(&ForcedFlatInputs {
            phase: self.active.phase,
            metadata_matches_selection: open_position.book.metadata_matches_selection(),
            last_chainlink_ts_ms: self.active.last_reference_ts_ms,
            now_ms,
            stale_chainlink_after_ms: self.config.forced_flat_stale_chainlink_ms,
            liquidity_available: open_position.book.liquidity_available,
            min_liquidity_required: self.config.forced_flat_thin_book_min_liquidity,
            fast_venue_incoherent: self.active.fast_venue_incoherent,
        })
        .into_iter()
        .collect()
    }

    fn current_realized_vol_at(&self, now_ms: u64) -> Option<f64> {
        self.pricing.realized_vol.current_vol_at(now_ms)
    }

    fn current_seconds_to_expiry_at(&self, now_ms: u64) -> Option<u64> {
        self.active.seconds_to_expiry_at(now_ms)
    }

    fn current_entry_pricing_inputs_at(
        &self,
        now_ms: u64,
    ) -> std::result::Result<EntryPricingInputs, Vec<EntryPricingBlockReason>> {
        let mut blocked_by = Vec::new();

        let spot_price = self
            .pricing
            .spot_price()
            .filter(|value| value.is_finite() && *value > 0.0);
        if spot_price.is_none() {
            blocked_by.push(EntryPricingBlockReason::SpotPriceMissing);
        }

        let strike_price = self
            .active
            .interval_open
            .filter(|value| value.is_finite() && *value > 0.0);
        if strike_price.is_none() {
            blocked_by.push(EntryPricingBlockReason::StrikePriceMissing);
        }

        let seconds_to_expiry = self.current_seconds_to_expiry_at(now_ms);
        if seconds_to_expiry.is_none() {
            blocked_by.push(EntryPricingBlockReason::SecondsToExpiryMissing);
        }

        let realized_vol = self
            .current_realized_vol_at(now_ms)
            .filter(|value| value.is_finite() && *value > 0.0);
        if realized_vol.is_none() {
            blocked_by.push(EntryPricingBlockReason::RealizedVolNotReady);
        }

        let theta_scaled_min_edge_bps = seconds_to_expiry.and_then(|seconds_to_expiry| {
            compute_theta_scaler(&ThetaScalerInputs {
                seconds_to_expiry,
                period_duration_secs: self.config.period_duration_secs,
                theta_decay_factor: self.config.theta_decay_factor,
            })
            .map(|theta| self.config.worst_case_ev_min_bps as f64 * theta)
        });
        if theta_scaled_min_edge_bps.is_none() {
            blocked_by.push(EntryPricingBlockReason::ThetaScalerUnavailable);
        }

        if !blocked_by.is_empty() {
            return Err(blocked_by);
        }

        Ok(EntryPricingInputs {
            spot_price: spot_price.expect("validated above"),
            strike_price: strike_price.expect("validated above"),
            seconds_to_expiry: seconds_to_expiry.expect("validated above"),
            realized_vol: realized_vol.expect("validated above"),
            theta_scaled_min_edge_bps: theta_scaled_min_edge_bps.expect("validated above"),
        })
    }

    fn current_fair_probability_up_at(&self, now_ms: u64) -> Option<f64> {
        let inputs = self.current_entry_pricing_inputs_at(now_ms).ok()?;
        compute_fair_probability_up(&FairProbabilityInputs {
            spot_price: inputs.spot_price,
            strike_price: inputs.strike_price,
            seconds_to_expiry: inputs.seconds_to_expiry,
            realized_vol: inputs.realized_vol,
            pricing_kurtosis: self.config.pricing_kurtosis,
        })
    }

    fn current_scaled_min_edge_bps_at(&self, now_ms: u64) -> Option<f64> {
        compute_theta_scaler(&ThetaScalerInputs {
            seconds_to_expiry: self.current_seconds_to_expiry_at(now_ms)?,
            period_duration_secs: self.config.period_duration_secs,
            theta_decay_factor: self.config.theta_decay_factor,
        })
        .map(|theta| self.config.worst_case_ev_min_bps as f64 * theta)
    }

    fn current_uncertainty_band_probability_at(
        &self,
        now_ms: u64,
        up_fee_bps: f64,
        down_fee_bps: f64,
    ) -> Option<f64> {
        let seconds_to_expiry = self.current_seconds_to_expiry_at(now_ms)?;
        self.uncertainty_band_probability_for_seconds(seconds_to_expiry, up_fee_bps, down_fee_bps)
    }

    fn uncertainty_band_probability_for_seconds(
        &self,
        seconds_to_expiry: u64,
        up_fee_bps: f64,
        down_fee_bps: f64,
    ) -> Option<f64> {
        let time_uncertainty_probability = if self.config.period_duration_secs == 0 {
            return None;
        } else {
            (1.0 - seconds_to_expiry as f64 / self.config.period_duration_secs as f64)
                .clamp(0.0, 1.0)
        };
        let fee_uncertainty_probability =
            (up_fee_bps.max(down_fee_bps) / BPS_DENOMINATOR).clamp(0.0, 1.0);

        uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability: self.pricing.last_lead_gap_probability.unwrap_or(0.0),
            jitter_penalty_probability: self.pricing.last_jitter_penalty_probability.unwrap_or(0.0),
            time_uncertainty_probability,
            fee_uncertainty_probability,
        })
    }

    fn entry_evaluation_log_fields_at(
        &self,
        now_ms: u64,
        submission: &EntrySubmissionDecision,
    ) -> EntryEvaluationLogFields {
        let evaluation = &submission.evaluation;
        let spot_venue_name = self
            .pricing
            .fast_spot
            .as_ref()
            .map(|spot| spot.venue_name.clone());
        let fast_venue_available = spot_venue_name.is_some();
        let (realized_vol_source_venue, realized_vol_source_ts_ms) =
            self.pricing.current_realized_vol_source_at(now_ms);

        EntryEvaluationLogFields {
            market_id: self.active.market_id.clone(),
            phase: self.active.phase,
            gate_blocked_by: evaluation.gate.blocked_by.clone(),
            pricing_blocked_by: evaluation.pricing_blocked_by.clone(),
            spot_price: self.pricing.spot_price(),
            spot_venue_name,
            reference_fair_value: self.pricing.last_reference_fair_value,
            interval_open: self.active.interval_open,
            seconds_to_expiry: self.current_seconds_to_expiry_at(now_ms),
            realized_vol: self.current_realized_vol_at(now_ms),
            realized_vol_source_venue,
            realized_vol_source_ts_ms,
            pricing_kurtosis: self.config.pricing_kurtosis,
            theta_decay_factor: self.config.theta_decay_factor,
            theta_scaled_min_edge_bps: evaluation
                .min_worst_case_ev_bps
                .or_else(|| self.current_scaled_min_edge_bps_at(now_ms)),
            fair_probability_up: evaluation.fair_probability_up,
            fair_probability_down: evaluation.fair_probability_up.map(|value| 1.0 - value),
            uncertainty_band_probability: evaluation.uncertainty_band_probability,
            uncertainty_band_live: evaluation.uncertainty_band_probability.is_some(),
            uncertainty_band_reason: if evaluation.uncertainty_band_probability.is_some() {
                "derived_from_lead_gap_jitter_time_and_fee"
            } else {
                "uncertainty_band_unavailable"
            },
            lead_agreement_corr: self.pricing.last_lead_agreement_corr,
            fast_venue_age_ms: self.pricing.last_fast_venue_age_ms,
            fast_venue_jitter_ms: self.pricing.last_fast_venue_jitter_ms,
            up_fee_bps: self.outcome_fee_bps(OutcomeSide::Up),
            down_fee_bps: self.outcome_fee_bps(OutcomeSide::Down),
            up_entry_cost: self.executable_entry_cost(OutcomeSide::Up),
            down_entry_cost: self.executable_entry_cost(OutcomeSide::Down),
            up_worst_case_ev_bps: evaluation.up_worst_case_ev_bps,
            down_worst_case_ev_bps: evaluation.down_worst_case_ev_bps,
            expected_ev_per_usdc: evaluation.expected_ev_per_usdc,
            max_position_usdc: self.config.max_position_usdc,
            risk_lambda: self.config.risk_lambda,
            book_impact_cap_bps: self.config.book_impact_cap_bps,
            book_impact_cap_usdc: evaluation.book_impact_cap_usdc,
            sized_notional_usdc: evaluation.sized_notional_usdc,
            selected_side: evaluation.selected_side,
            fast_venue_available,
            fast_venue_fallback_to_reference: !fast_venue_available
                && self.pricing.last_reference_fair_value.is_some(),
            lead_quality_policy_applied: self.pricing.lead_quality_policy_applied,
            lead_quality_reason: if self.pricing.fast_venue_incoherent {
                "no_fast_venue_cleared_lead_quality_thresholds"
            } else {
                "lead_quality_thresholds_applied_to_live_fast_spot_selection"
            },
            maker_rebate_available: false,
            maker_rebate_reason: "taker_fok_path_does_not_use_maker_rebate",
            category_available: false,
            category_reason: "market_category_not_visible_on_current_strategy_seam",
            final_fee_amount_known: false,
            final_fee_amount_reason: "final_fee_requires_side_price_and_size_selection",
            submission_instrument_id: submission.instrument_id,
            submission_order_side: submission.order_side,
            submission_price: submission.price,
            submission_quantity_value: submission.quantity_value,
            submission_client_order_id: submission.client_order_id,
            submission_blocked_reason: submission.blocked_reason,
        }
    }

    fn log_entry_evaluation(&self, now_ms: u64, submission: &EntrySubmissionDecision) {
        let fields = self.entry_evaluation_log_fields_at(now_ms, submission);
        let blocked = !fields.gate_blocked_by.is_empty() || !fields.pricing_blocked_by.is_empty();

        if blocked {
            log::warn!(
                "eth_chainlink_taker entry blocked: strategy_id={} reasons={:?}",
                self.config.strategy_id,
                fields.gate_blocked_by
            );
            if fields
                .gate_blocked_by
                .contains(&EntryBlockReason::FeesNotReady)
            {
                log::warn!(
                    "eth_chainlink_taker fee-rate unavailable: strategy_id={} entry remains fail-closed",
                    self.config.strategy_id
                );
            }
            log::warn!(
                "eth_chainlink_taker entry evaluation: strategy_id={} market_id={:?} phase={:?} gate_blocked_by={:?} pricing_blocked_by={:?} spot_price={:?} spot_venue_name={:?} reference_fair_value={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} theta_decay_factor={} theta_scaled_min_edge_bps={:?} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} uncertainty_band_live={} uncertainty_band_reason={} lead_agreement_corr={:?} fast_venue_age_ms={:?} fast_venue_jitter_ms={:?} up_fee_bps={:?} down_fee_bps={:?} up_entry_cost={:?} down_entry_cost={:?} up_worst_case_ev_bps={:?} down_worst_case_ev_bps={:?} expected_ev_per_usdc={:?} max_position_usdc={} risk_lambda={} book_impact_cap_bps={} book_impact_cap_usdc={:?} sized_notional_usdc={:?} selected_side={:?} fast_venue_available={} fast_venue_fallback_to_reference={} lead_quality_policy_applied={} lead_quality_reason={} maker_rebate_available={} maker_rebate_reason={} category_available={} category_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity_value={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
                self.config.strategy_id,
                fields.market_id,
                fields.phase,
                fields.gate_blocked_by,
                fields.pricing_blocked_by,
                fields.spot_price,
                fields.spot_venue_name,
                fields.reference_fair_value,
                fields.interval_open,
                fields.seconds_to_expiry,
                fields.realized_vol,
                fields.realized_vol_source_venue,
                fields.realized_vol_source_ts_ms,
                fields.pricing_kurtosis,
                fields.theta_decay_factor,
                fields.theta_scaled_min_edge_bps,
                fields.fair_probability_up,
                fields.fair_probability_down,
                fields.uncertainty_band_probability,
                fields.uncertainty_band_live,
                fields.uncertainty_band_reason,
                fields.lead_agreement_corr,
                fields.fast_venue_age_ms,
                fields.fast_venue_jitter_ms,
                fields.up_fee_bps,
                fields.down_fee_bps,
                fields.up_entry_cost,
                fields.down_entry_cost,
                fields.up_worst_case_ev_bps,
                fields.down_worst_case_ev_bps,
                fields.expected_ev_per_usdc,
                fields.max_position_usdc,
                fields.risk_lambda,
                fields.book_impact_cap_bps,
                fields.book_impact_cap_usdc,
                fields.sized_notional_usdc,
                fields.selected_side,
                fields.fast_venue_available,
                fields.fast_venue_fallback_to_reference,
                fields.lead_quality_policy_applied,
                fields.lead_quality_reason,
                fields.maker_rebate_available,
                fields.maker_rebate_reason,
                fields.category_available,
                fields.category_reason,
                fields.final_fee_amount_known,
                fields.final_fee_amount_reason,
                fields.submission_instrument_id,
                fields.submission_order_side,
                fields.submission_price,
                fields.submission_quantity_value,
                fields.submission_client_order_id,
                fields.submission_blocked_reason,
            );
        } else {
            log::info!(
                "eth_chainlink_taker entry evaluation: strategy_id={} market_id={:?} phase={:?} gate_blocked_by={:?} pricing_blocked_by={:?} spot_price={:?} spot_venue_name={:?} reference_fair_value={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} theta_decay_factor={} theta_scaled_min_edge_bps={:?} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} uncertainty_band_live={} uncertainty_band_reason={} lead_agreement_corr={:?} fast_venue_age_ms={:?} fast_venue_jitter_ms={:?} up_fee_bps={:?} down_fee_bps={:?} up_entry_cost={:?} down_entry_cost={:?} up_worst_case_ev_bps={:?} down_worst_case_ev_bps={:?} expected_ev_per_usdc={:?} max_position_usdc={} risk_lambda={} book_impact_cap_bps={} book_impact_cap_usdc={:?} sized_notional_usdc={:?} selected_side={:?} fast_venue_available={} fast_venue_fallback_to_reference={} lead_quality_policy_applied={} lead_quality_reason={} maker_rebate_available={} maker_rebate_reason={} category_available={} category_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity_value={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
                self.config.strategy_id,
                fields.market_id,
                fields.phase,
                fields.gate_blocked_by,
                fields.pricing_blocked_by,
                fields.spot_price,
                fields.spot_venue_name,
                fields.reference_fair_value,
                fields.interval_open,
                fields.seconds_to_expiry,
                fields.realized_vol,
                fields.realized_vol_source_venue,
                fields.realized_vol_source_ts_ms,
                fields.pricing_kurtosis,
                fields.theta_decay_factor,
                fields.theta_scaled_min_edge_bps,
                fields.fair_probability_up,
                fields.fair_probability_down,
                fields.uncertainty_band_probability,
                fields.uncertainty_band_live,
                fields.uncertainty_band_reason,
                fields.lead_agreement_corr,
                fields.fast_venue_age_ms,
                fields.fast_venue_jitter_ms,
                fields.up_fee_bps,
                fields.down_fee_bps,
                fields.up_entry_cost,
                fields.down_entry_cost,
                fields.up_worst_case_ev_bps,
                fields.down_worst_case_ev_bps,
                fields.expected_ev_per_usdc,
                fields.max_position_usdc,
                fields.risk_lambda,
                fields.book_impact_cap_bps,
                fields.book_impact_cap_usdc,
                fields.sized_notional_usdc,
                fields.selected_side,
                fields.fast_venue_available,
                fields.fast_venue_fallback_to_reference,
                fields.lead_quality_policy_applied,
                fields.lead_quality_reason,
                fields.maker_rebate_available,
                fields.maker_rebate_reason,
                fields.category_available,
                fields.category_reason,
                fields.final_fee_amount_known,
                fields.final_fee_amount_reason,
                fields.submission_instrument_id,
                fields.submission_order_side,
                fields.submission_price,
                fields.submission_quantity_value,
                fields.submission_client_order_id,
                fields.submission_blocked_reason,
            );
        }
    }

    fn write_bolt_v3_entry_evaluation(
        &mut self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
    ) -> Result<()> {
        let Some(evidence) = self.context.bolt_v3_decision_evidence.clone() else {
            return Ok(());
        };

        let Some(facts) = self.entry_evaluation_decision_facts(now_ms, decision)? else {
            return Ok(());
        };
        let decision_trace_id = self.active_decision_trace_id();
        let ts = unix_nanos_from_millis(now_ms)?;
        evidence.write_entry_evaluation(&decision_trace_id, facts, ts, ts)
    }

    fn write_bolt_v3_market_selection_result(
        &mut self,
        snapshot: &RuntimeSelectionSnapshot,
    ) -> Result<()> {
        let Some(evidence) = self.context.bolt_v3_decision_evidence.clone() else {
            return Ok(());
        };
        let Some(target_context) = self.context.bolt_v3_market_selection_context.as_ref() else {
            return Ok(());
        };
        let Some(facts) = market_selection_result_facts(snapshot, target_context)? else {
            return Ok(());
        };
        let decision_trace_id = self.active_decision_trace_id();
        let ts = unix_nanos_from_millis(snapshot.published_at_ms)?;
        evidence.write_market_selection_result(&decision_trace_id, facts, ts, ts)
    }

    fn write_bolt_v3_entry_pre_submit_rejection(
        &mut self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
    ) -> Result<()> {
        let Some(evidence) = self.context.bolt_v3_decision_evidence.clone() else {
            return Ok(());
        };

        let Some(facts) = entry_pre_submit_rejection_facts(decision)? else {
            return Ok(());
        };
        let decision_trace_id = self.active_decision_trace_id();
        let ts = unix_nanos_from_millis(now_ms)?;
        evidence.write_entry_pre_submit_rejection(&decision_trace_id, facts, ts, ts)
    }

    fn entry_evaluation_decision_facts(
        &self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
    ) -> Result<Option<BoltV3EntryEvaluationFacts>> {
        let no_action_reason = entry_no_action_reason(decision);
        if decision.blocked_reason.is_some() && no_action_reason.is_none() {
            return Ok(None);
        }

        let seconds_to_market_end = self.current_seconds_to_expiry_at(now_ms).ok_or_else(|| {
            anyhow::anyhow!("entry evaluation event requires seconds to market end")
        })?;

        let archetype_metrics = json!({
            "expected_ev_per_usdc": decision.evaluation.expected_ev_per_usdc,
            "up_worst_case_ev_bps": decision.evaluation.up_worst_case_ev_bps,
            "down_worst_case_ev_bps": decision.evaluation.down_worst_case_ev_bps,
            "sized_notional_usdc": decision.evaluation.sized_notional_usdc,
            "book_impact_cap_usdc": decision.evaluation.book_impact_cap_usdc,
        });

        if let Some(reason) = no_action_reason {
            return Ok(Some(BoltV3EntryEvaluationFacts {
                updown_side: None,
                entry_decision: "no_action".to_string(),
                entry_no_action_reason: Some(reason.to_string()),
                seconds_to_market_end,
                has_selected_market_open_orders: false,
                updown_market_mechanical_outcome: "accepted".to_string(),
                updown_market_mechanical_rejection_reason: None,
                entry_filled_notional: 0.0,
                open_entry_notional: 0.0,
                strategy_remaining_entry_capacity: self.config.max_position_usdc,
                archetype_metrics,
            }));
        }

        let selected_side = decision
            .evaluation
            .selected_side
            .ok_or_else(|| anyhow::anyhow!("entry evaluation event requires selected side"))?;

        Ok(Some(BoltV3EntryEvaluationFacts {
            updown_side: Some(outcome_side_as_decision_fact(selected_side).to_string()),
            entry_decision: "enter".to_string(),
            entry_no_action_reason: None,
            seconds_to_market_end,
            has_selected_market_open_orders: false,
            updown_market_mechanical_outcome: "accepted".to_string(),
            updown_market_mechanical_rejection_reason: None,
            entry_filled_notional: 0.0,
            open_entry_notional: 0.0,
            strategy_remaining_entry_capacity: self.config.max_position_usdc,
            archetype_metrics,
        }))
    }

    fn outcome_fee_bps(&self, side: OutcomeSide) -> Option<f64> {
        let token_id = match side {
            OutcomeSide::Up => self.active.outcome_fees.up_token_id.as_deref(),
            OutcomeSide::Down => self.active.outcome_fees.down_token_id.as_deref(),
        }?;
        self.context.fee_provider.fee_bps(token_id)?.to_f64()
    }

    fn executable_entry_cost(&self, side: OutcomeSide) -> Option<f64> {
        match side {
            OutcomeSide::Up => self.active.books.up.best_ask,
            OutcomeSide::Down => self.active.books.down.best_ask,
        }
        .filter(|value| value.is_finite() && *value > 0.0)
    }

    fn submission_entry_price(&self, side: OutcomeSide) -> Option<f64> {
        match side {
            OutcomeSide::Up => self.active.books.up.best_ask,
            OutcomeSide::Down => self.active.books.down.best_ask,
        }
        .filter(|value| value.is_finite() && *value > 0.0)
    }

    fn visible_book_notional_cap_usdc(&self, side: OutcomeSide) -> Option<f64> {
        let capped_execution = match side {
            OutcomeSide::Up => self
                .active
                .books
                .up
                .max_buy_execution_within_vwap_slippage_bps(self.config.book_impact_cap_bps),
            OutcomeSide::Down => self
                .active
                .books
                .down
                .max_buy_execution_within_vwap_slippage_bps(self.config.book_impact_cap_bps),
        }
        .filter(|execution| execution.quantity.is_finite() && execution.quantity > 0.0)?;
        Some(match side {
            OutcomeSide::Up => capped_execution.quantity * capped_execution.vwap_price,
            OutcomeSide::Down => capped_execution.quantity * capped_execution.vwap_price,
        })
    }

    fn instrument_id_for_side(&self, side: OutcomeSide) -> Option<InstrumentId> {
        match side {
            OutcomeSide::Up => self.active.books.up.instrument_id,
            OutcomeSide::Down => self.active.books.down.instrument_id,
        }
    }

    fn current_instrument(&self, instrument_id: InstrumentId) -> Option<InstrumentAny> {
        self.core.trader_id()?;
        let cache = self.cache();
        cache.instrument(&instrument_id).cloned()
    }

    fn infer_outcome_side_from_instrument_id(instrument_id: InstrumentId) -> Option<OutcomeSide> {
        let instrument = instrument_id.to_string();
        if instrument.contains("-UP.") {
            Some(OutcomeSide::Up)
        } else if instrument.contains("-DOWN.") {
            Some(OutcomeSide::Down)
        } else {
            None
        }
    }

    fn pending_entry_context_for(&self, instrument_id: InstrumentId) -> Option<PendingEntryState> {
        let pending = self.pending_entry()?.clone();
        if pending.instrument_id != instrument_id {
            return None;
        }

        Some(pending)
    }

    fn build_open_position_state(
        &self,
        preserved: Option<&OpenPositionState>,
        pending_context: Option<&PendingEntryState>,
        spec: PositionMaterializationSpec,
        trust_pending_outcome_side: bool,
    ) -> OpenPositionState {
        OpenPositionState {
            market_id: preserved
                .and_then(|position| position.market_id.clone())
                .or_else(|| pending_context.and_then(|pending| pending.market_id.clone()))
                .or_else(|| self.active.market_id.clone()),
            instrument_id: spec.instrument_id,
            position_id: spec.position_id,
            outcome_side: preserved
                .and_then(|position| position.outcome_side)
                .or_else(|| {
                    if trust_pending_outcome_side {
                        pending_context.and_then(|pending| pending.outcome_side)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    infer_strategy_outcome_side(
                        spec.entry_order_side,
                        spec.side,
                        spec.instrument_id,
                    )
                }),
            outcome_fees: preserved
                .map(|position| position.outcome_fees.clone())
                .or_else(|| pending_context.map(|pending| pending.outcome_fees.clone()))
                .unwrap_or_else(|| self.active.outcome_fees.clone()),
            historical_entry_fee_bps: preserved
                .and_then(|position| position.historical_entry_fee_bps)
                .or_else(|| pending_context.and_then(|pending| pending.historical_entry_fee_bps)),
            entry_order_side: spec.entry_order_side,
            side: spec.side,
            quantity: spec.quantity,
            avg_px_open: spec.avg_px_open,
            interval_open: preserved
                .and_then(|position| position.interval_open)
                .or_else(|| pending_context.and_then(|pending| pending.interval_open))
                .or(self.active.interval_open),
            selection_published_at_ms: preserved
                .and_then(|position| position.selection_published_at_ms)
                .or_else(|| pending_context.and_then(|pending| pending.selection_published_at_ms))
                .or(self.active.selection_published_at_ms),
            seconds_to_expiry_at_selection: preserved
                .and_then(|position| position.seconds_to_expiry_at_selection)
                .or_else(|| {
                    pending_context.and_then(|pending| pending.seconds_to_expiry_at_selection)
                })
                .or(self.active.seconds_to_expiry_at_selection),
            book: preserved
                .map(|position| position.book.clone())
                .or_else(|| pending_context.map(|pending| pending.book.clone()))
                .unwrap_or_else(|| OutcomeBookState::from_instrument_id(spec.instrument_id)),
        }
    }

    fn materialize_position_from_event(
        &mut self,
        instrument_id: InstrumentId,
        position_id: PositionId,
        entry_order_side: OrderSide,
        side: PositionSide,
        quantity: Quantity,
        avg_px_open: f64,
    ) {
        let preserved = self
            .managed_position()
            .filter(|managed| {
                managed.position.position_id == position_id
                    && managed.position.instrument_id == instrument_id
            })
            .map(|managed| managed.position.clone());
        let pending_context = self.pending_entry_context_for(instrument_id);
        let pending_matches = pending_context.is_some();
        let observed_open_side = is_observed_open_side(side);
        let tradable_position_supported =
            supports_strategy_managed_position(entry_order_side, side);

        if !observed_open_side {
            self.exposure = if let Some(pending) = pending_context {
                ExposureState::EntryReconcilePending {
                    pending,
                    reason: EntryReconcileReason::InvalidObservedPosition {
                        entry_order_side,
                        side,
                    },
                }
            } else {
                ExposureState::BlindRecovery(BlindRecoveryState {
                    reason: BlindRecoveryReason::InvalidLivePosition {
                        entry_order_side,
                        side,
                    },
                })
            };
            log::error!(
                "eth_chainlink_taker position event carried unsupported position side: strategy_id={} instrument_id={} position_id={} entry_order_side={:?} side={:?}",
                self.config.strategy_id,
                instrument_id,
                position_id,
                entry_order_side,
                side,
            );
            self.refresh_book_subscriptions_for_current_state();
            return;
        }

        if !tradable_position_supported {
            log::error!(
                "eth_chainlink_taker quarantining unsupported observed position contract: strategy_id={} instrument_id={} entry_order_side={:?} side={:?}",
                self.config.strategy_id,
                instrument_id,
                entry_order_side,
                side,
            );
            self.set_unsupported_observed_exposure(
                self.build_open_position_state(
                    preserved.as_ref(),
                    pending_context.as_ref(),
                    PositionMaterializationSpec {
                        instrument_id,
                        position_id,
                        entry_order_side,
                        side,
                        quantity,
                        avg_px_open,
                    },
                    false,
                ),
                UnsupportedObservedReason::LiveUnsupportedContract,
            );
            return;
        }

        let origin = self
            .managed_position()
            .filter(|managed| {
                managed.position.position_id == position_id
                    && managed.position.instrument_id == instrument_id
            })
            .map(|managed| managed.origin)
            .unwrap_or(ManagedPositionOrigin::StrategyEntry);
        self.exposure = ExposureState::Managed(ManagedPositionState {
            position: self.build_open_position_state(
                preserved.as_ref(),
                pending_context.as_ref(),
                PositionMaterializationSpec {
                    instrument_id,
                    position_id,
                    entry_order_side,
                    side,
                    quantity,
                    avg_px_open,
                },
                pending_matches,
            ),
            origin,
        });
        self.sync_exposure_context_from_active();
        self.refresh_book_subscriptions_for_current_state();
    }

    fn seconds_to_expiry_from_selection(
        selection_published_at_ms: Option<u64>,
        seconds_to_expiry_at_selection: Option<u64>,
        now_ms: u64,
    ) -> Option<u64> {
        let published_at_ms = selection_published_at_ms?;
        let seconds_to_expiry_at_selection = seconds_to_expiry_at_selection?;
        let elapsed_seconds = now_ms.saturating_sub(published_at_ms) / MILLIS_PER_SECOND_U64;
        Some(seconds_to_expiry_at_selection.saturating_sub(elapsed_seconds))
    }

    fn sync_exposure_context_from_active(&mut self) {
        let active_market_id = self.active.market_id.clone();
        let active_outcome_fees = self.active.outcome_fees.clone();
        let active_interval_open = self.active.interval_open;
        let active_selection_published_at_ms = self.active.selection_published_at_ms;
        let active_seconds_to_expiry_at_selection = self.active.seconds_to_expiry_at_selection;
        let active_up_instrument_id = self.active.books.up.instrument_id;
        let active_down_instrument_id = self.active.books.down.instrument_id;
        let active_up_book = self.active.books.up.clone();
        let active_down_book = self.active.books.down.clone();
        let Some(open_position) = self.tracked_observed_position_mut() else {
            return;
        };

        if active_up_instrument_id == Some(open_position.instrument_id) {
            open_position.market_id = active_market_id.clone();
            open_position.outcome_side = Some(OutcomeSide::Up);
            open_position.outcome_fees = active_outcome_fees.clone();
            open_position.interval_open = active_interval_open;
            open_position.selection_published_at_ms = active_selection_published_at_ms;
            open_position.seconds_to_expiry_at_selection = active_seconds_to_expiry_at_selection;
            open_position.book = active_up_book;
        } else if active_down_instrument_id == Some(open_position.instrument_id) {
            open_position.market_id = active_market_id;
            open_position.outcome_side = Some(OutcomeSide::Down);
            open_position.outcome_fees = active_outcome_fees;
            open_position.interval_open = active_interval_open;
            open_position.selection_published_at_ms = active_selection_published_at_ms;
            open_position.seconds_to_expiry_at_selection = active_seconds_to_expiry_at_selection;
            open_position.book = active_down_book;
        }
    }

    fn desired_book_subscriptions_for_active(&self) -> OutcomeBookSubscriptions {
        let mut next = OutcomeBookSubscriptions {
            up_instrument_id: self.active.books.up.instrument_id,
            down_instrument_id: self.active.books.down.instrument_id,
            tracked_position_instrument_id: None,
        };

        if let Some(open_position) = self.tracked_observed_position()
            && next.up_instrument_id != Some(open_position.instrument_id)
            && next.down_instrument_id != Some(open_position.instrument_id)
        {
            next.tracked_position_instrument_id = Some(open_position.instrument_id);
        } else if let Some(pending_entry_instrument_id) =
            self.pending_entry().map(|pending| pending.instrument_id)
            && next.up_instrument_id != Some(pending_entry_instrument_id)
            && next.down_instrument_id != Some(pending_entry_instrument_id)
        {
            next.tracked_position_instrument_id = Some(pending_entry_instrument_id);
        }

        next
    }

    fn refresh_book_subscriptions_for_current_state(&mut self) {
        let next = self.desired_book_subscriptions_for_active();
        if should_replace_book_subscriptions(&self.book_subscriptions, &next) {
            self.replace_book_subscriptions(next);
        }
    }

    fn open_position_outcome_side(&self) -> Option<OutcomeSide> {
        self.managed_position()
            .and_then(|position| position.position.outcome_side)
    }

    fn open_position_effective_entry_cost(&self) -> Option<f64> {
        let open_position = &self.managed_position()?.position;
        managed_position_effective_entry_cost(open_position)
    }

    fn current_exit_order_for_open_position(&self) -> Option<(OrderSide, f64)> {
        let open_position = &self.managed_position()?.position;
        managed_position_exit_order(open_position)
    }

    fn current_exit_value_for_open_position(&self) -> Option<f64> {
        let open_position = &self.managed_position()?.position;
        managed_position_exit_value(open_position)
    }

    fn current_position_market_id(&self) -> Option<String> {
        self.exposure.current_position_market_id()
    }

    fn current_position_seconds_to_expiry_at(&self, now_ms: u64) -> Option<u64> {
        let open_position = &self.managed_position()?.position;
        Self::seconds_to_expiry_from_selection(
            open_position.selection_published_at_ms,
            open_position.seconds_to_expiry_at_selection,
            now_ms,
        )
    }

    fn current_position_fair_probability_up_at(&self, now_ms: u64) -> Option<f64> {
        let open_position = &self.managed_position()?.position;
        let spot_price = self
            .pricing
            .spot_price()
            .filter(|value| value.is_finite() && *value > 0.0)?;
        let strike_price = open_position
            .interval_open
            .filter(|value| value.is_finite() && *value > 0.0)?;
        let seconds_to_expiry = self.current_position_seconds_to_expiry_at(now_ms)?;
        let realized_vol = self
            .current_realized_vol_at(now_ms)
            .filter(|value| value.is_finite() && *value > 0.0)?;
        compute_fair_probability_up(&FairProbabilityInputs {
            spot_price,
            strike_price,
            seconds_to_expiry,
            realized_vol,
            pricing_kurtosis: self.config.pricing_kurtosis,
        })
    }

    fn current_position_uncertainty_band_probability_at(&self, now_ms: u64) -> Option<f64> {
        let seconds_to_expiry = self.current_position_seconds_to_expiry_at(now_ms)?;
        let up_fee_bps = self.position_outcome_fee_bps(OutcomeSide::Up)?;
        let down_fee_bps = self.position_outcome_fee_bps(OutcomeSide::Down)?;
        self.uncertainty_band_probability_for_seconds(seconds_to_expiry, up_fee_bps, down_fee_bps)
    }

    fn current_hold_ev_bps_at(&self, now_ms: u64, side: OutcomeSide) -> Option<f64> {
        let open_position = &self.managed_position()?.position;
        let spot_price = self
            .pricing
            .spot_price()
            .filter(|value| value.is_finite() && *value > 0.0)?;
        let strike_price = open_position
            .interval_open
            .filter(|value| value.is_finite() && *value > 0.0)?;
        let seconds_to_expiry = Self::seconds_to_expiry_from_selection(
            open_position.selection_published_at_ms,
            open_position.seconds_to_expiry_at_selection,
            now_ms,
        )?;
        let realized_vol = self
            .current_realized_vol_at(now_ms)
            .filter(|value| value.is_finite() && *value > 0.0)?;
        let fair_probability_up = compute_fair_probability_up(&FairProbabilityInputs {
            spot_price,
            strike_price,
            seconds_to_expiry,
            realized_vol,
            pricing_kurtosis: self.config.pricing_kurtosis,
        })?;
        let up_fee_bps = self.position_outcome_fee_bps(OutcomeSide::Up)?;
        let down_fee_bps = self.position_outcome_fee_bps(OutcomeSide::Down)?;
        let uncertainty_band_probability = self.uncertainty_band_probability_for_seconds(
            seconds_to_expiry,
            up_fee_bps,
            down_fee_bps,
        )?;
        let effective_entry_cost = self.open_position_effective_entry_cost()?;
        let fee_bps = self.open_position_historical_entry_fee_bps()?;

        compute_worst_case_ev_bps(
            side,
            &WorstCaseEvInputs {
                fair_probability: Some(fair_probability_up),
                uncertainty_band_probability,
                executable_entry_cost: effective_entry_cost,
                fee_bps: Some(fee_bps),
            },
        )
    }

    fn current_exit_ev_bps_at(&self, side: OutcomeSide) -> Option<f64> {
        let effective_entry_cost = self.open_position_effective_entry_cost()?;
        let historical_entry_fee_bps = self.open_position_historical_entry_fee_bps()?;
        let current_exit_fee_bps = self.position_outcome_fee_bps(side)?;
        let total_entry_cost =
            effective_entry_cost * (1.0 + historical_entry_fee_bps / BPS_DENOMINATOR);
        if !total_entry_cost.is_finite() || total_entry_cost <= 0.0 {
            return None;
        }

        let current_exit_value = self.current_exit_value_for_open_position()?;
        let net_exit_value = current_exit_value * (1.0 - current_exit_fee_bps / BPS_DENOMINATOR);
        if !net_exit_value.is_finite() || net_exit_value <= 0.0 {
            return None;
        }

        Some(((net_exit_value - total_entry_cost) / total_entry_cost) * BPS_DENOMINATOR)
    }

    fn open_position_historical_entry_fee_bps(&self) -> Option<f64> {
        self.managed_position()?.position.historical_entry_fee_bps
    }

    fn historical_entry_fee_log_fields(&self) -> (bool, &'static str) {
        let Some(managed_position) = self.managed_position() else {
            return (false, "no_managed_position");
        };

        if managed_position.position.historical_entry_fee_bps.is_some() {
            (true, "captured_from_strategy_entry_state")
        } else if managed_position.origin == ManagedPositionOrigin::RecoveryBootstrap {
            (
                false,
                "recovery_bootstrap_position_missing_original_fee_rate",
            )
        } else {
            (false, "position_state_missing_original_fee_rate")
        }
    }

    fn position_outcome_fee_bps(&self, side: OutcomeSide) -> Option<f64> {
        let open_position = &self.managed_position()?.position;
        let token_id = match side {
            OutcomeSide::Up => open_position.outcome_fees.up_token_id.as_deref(),
            OutcomeSide::Down => open_position.outcome_fees.down_token_id.as_deref(),
        }?;
        self.context.fee_provider.fee_bps(token_id)?.to_f64()
    }

    fn exit_evaluation_at(&self, now_ms: u64) -> ExitEvaluation {
        let mut evaluation = ExitEvaluation {
            position_outcome_side: self.open_position_outcome_side(),
            forced_flat_reasons: self.position_forced_flat_reasons_at(now_ms),
            hold_ev_bps: None,
            exit_ev_bps: None,
            exit_decision: None,
            blocked_reason: None,
        };

        if self.managed_position().is_none() {
            evaluation.blocked_reason = Some("no_open_position");
            return evaluation;
        }
        if self.exposure.exit_pending().is_some() {
            evaluation.blocked_reason = Some("exit_already_pending");
            return evaluation;
        }

        if !evaluation.forced_flat_reasons.is_empty() {
            evaluation.exit_decision = Some(ExitDecision::Exit);
            return evaluation;
        }

        let Some(position_outcome_side) = evaluation.position_outcome_side else {
            evaluation.exit_decision = Some(ExitDecision::ExitFailClosed);
            return evaluation;
        };

        evaluation.hold_ev_bps = self.current_hold_ev_bps_at(now_ms, position_outcome_side);
        evaluation.exit_ev_bps = self.current_exit_ev_bps_at(position_outcome_side);
        evaluation.exit_decision = Some(evaluate_exit_decision(
            evaluation.hold_ev_bps,
            evaluation.exit_ev_bps,
            self.config.exit_hysteresis_bps as f64,
        ));
        evaluation
    }

    fn exit_submission_decision_at(&self, now_ms: u64) -> ExitSubmissionDecision {
        let evaluation = self.exit_evaluation_at(now_ms);
        let mut decision = ExitSubmissionDecision {
            evaluation: evaluation.clone(),
            instrument_id: None,
            order_side: None,
            price: None,
            quantity: None,
            client_order_id: None,
            blocked_reason: evaluation.blocked_reason,
            forced_flat_reasons: evaluation.forced_flat_reasons.clone(),
        };

        let Some(exit_decision) = evaluation.exit_decision else {
            decision.blocked_reason = Some("exit_decision_unavailable");
            return decision;
        };
        if exit_decision == ExitDecision::Hold {
            decision.blocked_reason = Some("exit_hold");
            return decision;
        }

        let Some(open_position) = self.managed_position().map(|managed| &managed.position) else {
            decision.blocked_reason = Some("open_position_missing");
            return decision;
        };
        if !open_position.quantity.as_f64().is_finite() || open_position.quantity.as_f64() <= 0.0 {
            decision.blocked_reason = Some("exit_quantity_not_positive");
            return decision;
        }
        decision.instrument_id = Some(open_position.instrument_id);
        decision.order_side = match open_position.side {
            PositionSide::Long => Some(OrderSide::Sell),
            _ => None,
        };
        decision.quantity = Some(open_position.quantity);

        let Some((order_side, price)) = self.current_exit_order_for_open_position() else {
            decision.blocked_reason = Some("exit_price_missing");
            return decision;
        };
        decision.order_side = Some(order_side);
        decision.price = Some(price);
        decision.blocked_reason = None;
        decision
    }

    fn exit_evaluation_log_fields_at(
        &self,
        now_ms: u64,
        decision: &ExitSubmissionDecision,
    ) -> ExitEvaluationLogFields {
        let open_position = self.managed_position().map(|managed| &managed.position);
        let (historical_entry_fee_rate_known, historical_entry_fee_rate_reason) =
            self.historical_entry_fee_log_fields();
        let (realized_vol_source_venue, realized_vol_source_ts_ms) =
            self.pricing.current_realized_vol_source_at(now_ms);
        ExitEvaluationLogFields {
            market_id: self.current_position_market_id(),
            phase: self.active.phase,
            position_outcome_side: decision.evaluation.position_outcome_side,
            position_id: open_position.map(|position| position.position_id),
            position_instrument_id: open_position.map(|position| position.instrument_id),
            position_quantity: open_position.map(|position| position.quantity),
            position_avg_px_open: open_position.map(|position| position.avg_px_open),
            forced_flat_reasons: decision.forced_flat_reasons.clone(),
            spot_price: self.pricing.spot_price(),
            spot_venue_name: self
                .pricing
                .fast_spot
                .as_ref()
                .map(|spot| spot.venue_name.clone()),
            reference_fair_value: self.pricing.last_reference_fair_value,
            interval_open: open_position.and_then(|position| position.interval_open),
            seconds_to_expiry: self.current_position_seconds_to_expiry_at(now_ms),
            realized_vol: self.current_realized_vol_at(now_ms),
            realized_vol_source_venue,
            realized_vol_source_ts_ms,
            pricing_kurtosis: self.config.pricing_kurtosis,
            exit_hysteresis_bps: self.config.exit_hysteresis_bps,
            fair_probability_up: self.current_position_fair_probability_up_at(now_ms),
            fair_probability_down: self
                .current_position_fair_probability_up_at(now_ms)
                .map(|value| 1.0 - value),
            uncertainty_band_probability: self
                .current_position_uncertainty_band_probability_at(now_ms),
            up_fee_bps: self.position_outcome_fee_bps(OutcomeSide::Up),
            down_fee_bps: self.position_outcome_fee_bps(OutcomeSide::Down),
            hold_ev_bps: decision.evaluation.hold_ev_bps,
            exit_ev_bps: decision.evaluation.exit_ev_bps,
            exit_decision: decision.evaluation.exit_decision,
            historical_entry_fee_rate_known,
            historical_entry_fee_rate_reason,
            maker_rebate_available: false,
            maker_rebate_reason: "taker_fok_path_does_not_use_maker_rebate",
            category_available: false,
            category_reason: "market_category_not_visible_on_current_strategy_seam",
            final_fee_amount_known: false,
            final_fee_amount_reason: "final_fee_requires_side_price_size_and_actual_fill",
            submission_instrument_id: decision.instrument_id,
            submission_order_side: decision.order_side,
            submission_price: decision.price,
            submission_quantity: decision.quantity,
            submission_client_order_id: decision.client_order_id,
            submission_blocked_reason: decision.blocked_reason,
        }
    }

    fn log_exit_evaluation(&self, now_ms: u64, decision: &ExitSubmissionDecision) {
        let fields = self.exit_evaluation_log_fields_at(now_ms, decision);
        let blocked = fields.submission_blocked_reason.is_some();
        if blocked {
            if should_warn_on_exit_submission_block(fields.submission_blocked_reason) {
                log::warn!(
                    "eth_chainlink_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_fair_value={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} up_fee_bps={:?} down_fee_bps={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} historical_entry_fee_rate_known={} historical_entry_fee_rate_reason={} maker_rebate_available={} maker_rebate_reason={} category_available={} category_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
                    self.config.strategy_id,
                    fields.market_id,
                    fields.phase,
                    fields.position_outcome_side,
                    fields.position_id,
                    fields.position_instrument_id,
                    fields.position_quantity,
                    fields.position_avg_px_open,
                    fields.forced_flat_reasons,
                    fields.spot_price,
                    fields.spot_venue_name,
                    fields.reference_fair_value,
                    fields.interval_open,
                    fields.seconds_to_expiry,
                    fields.realized_vol,
                    fields.realized_vol_source_venue,
                    fields.realized_vol_source_ts_ms,
                    fields.pricing_kurtosis,
                    fields.exit_hysteresis_bps,
                    fields.fair_probability_up,
                    fields.fair_probability_down,
                    fields.uncertainty_band_probability,
                    fields.up_fee_bps,
                    fields.down_fee_bps,
                    fields.hold_ev_bps,
                    fields.exit_ev_bps,
                    fields.exit_decision,
                    fields.historical_entry_fee_rate_known,
                    fields.historical_entry_fee_rate_reason,
                    fields.maker_rebate_available,
                    fields.maker_rebate_reason,
                    fields.category_available,
                    fields.category_reason,
                    fields.final_fee_amount_known,
                    fields.final_fee_amount_reason,
                    fields.submission_instrument_id,
                    fields.submission_order_side,
                    fields.submission_price,
                    fields.submission_quantity,
                    fields.submission_client_order_id,
                    fields.submission_blocked_reason,
                );
            } else {
                log::debug!(
                    "eth_chainlink_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_fair_value={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} up_fee_bps={:?} down_fee_bps={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} historical_entry_fee_rate_known={} historical_entry_fee_rate_reason={} maker_rebate_available={} maker_rebate_reason={} category_available={} category_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
                    self.config.strategy_id,
                    fields.market_id,
                    fields.phase,
                    fields.position_outcome_side,
                    fields.position_id,
                    fields.position_instrument_id,
                    fields.position_quantity,
                    fields.position_avg_px_open,
                    fields.forced_flat_reasons,
                    fields.spot_price,
                    fields.spot_venue_name,
                    fields.reference_fair_value,
                    fields.interval_open,
                    fields.seconds_to_expiry,
                    fields.realized_vol,
                    fields.realized_vol_source_venue,
                    fields.realized_vol_source_ts_ms,
                    fields.pricing_kurtosis,
                    fields.exit_hysteresis_bps,
                    fields.fair_probability_up,
                    fields.fair_probability_down,
                    fields.uncertainty_band_probability,
                    fields.up_fee_bps,
                    fields.down_fee_bps,
                    fields.hold_ev_bps,
                    fields.exit_ev_bps,
                    fields.exit_decision,
                    fields.historical_entry_fee_rate_known,
                    fields.historical_entry_fee_rate_reason,
                    fields.maker_rebate_available,
                    fields.maker_rebate_reason,
                    fields.category_available,
                    fields.category_reason,
                    fields.final_fee_amount_known,
                    fields.final_fee_amount_reason,
                    fields.submission_instrument_id,
                    fields.submission_order_side,
                    fields.submission_price,
                    fields.submission_quantity,
                    fields.submission_client_order_id,
                    fields.submission_blocked_reason,
                );
            }
        } else {
            log::info!(
                "eth_chainlink_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_fair_value={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} up_fee_bps={:?} down_fee_bps={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} historical_entry_fee_rate_known={} historical_entry_fee_rate_reason={} maker_rebate_available={} maker_rebate_reason={} category_available={} category_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
                self.config.strategy_id,
                fields.market_id,
                fields.phase,
                fields.position_outcome_side,
                fields.position_id,
                fields.position_instrument_id,
                fields.position_quantity,
                fields.position_avg_px_open,
                fields.forced_flat_reasons,
                fields.spot_price,
                fields.spot_venue_name,
                fields.reference_fair_value,
                fields.interval_open,
                fields.seconds_to_expiry,
                fields.realized_vol,
                fields.realized_vol_source_venue,
                fields.realized_vol_source_ts_ms,
                fields.pricing_kurtosis,
                fields.exit_hysteresis_bps,
                fields.fair_probability_up,
                fields.fair_probability_down,
                fields.uncertainty_band_probability,
                fields.up_fee_bps,
                fields.down_fee_bps,
                fields.hold_ev_bps,
                fields.exit_ev_bps,
                fields.exit_decision,
                fields.historical_entry_fee_rate_known,
                fields.historical_entry_fee_rate_reason,
                fields.maker_rebate_available,
                fields.maker_rebate_reason,
                fields.category_available,
                fields.category_reason,
                fields.final_fee_amount_known,
                fields.final_fee_amount_reason,
                fields.submission_instrument_id,
                fields.submission_order_side,
                fields.submission_price,
                fields.submission_quantity,
                fields.submission_client_order_id,
                fields.submission_blocked_reason,
            );
        }
    }

    fn write_bolt_v3_exit_evaluation(
        &mut self,
        now_ms: u64,
        decision: &ExitSubmissionDecision,
    ) -> Result<()> {
        let Some(evidence) = self.context.bolt_v3_decision_evidence.clone() else {
            return Ok(());
        };
        let Some(facts) = self.exit_evaluation_decision_facts(decision)? else {
            return Ok(());
        };
        let decision_trace_id = self.active_decision_trace_id();
        let ts = unix_nanos_from_millis(now_ms)?;
        evidence.write_exit_evaluation(&decision_trace_id, facts, ts, ts)
    }

    fn write_bolt_v3_exit_pre_submit_rejection(
        &mut self,
        now_ms: u64,
        decision: &ExitSubmissionDecision,
    ) -> Result<()> {
        let Some(evidence) = self.context.bolt_v3_decision_evidence.clone() else {
            return Ok(());
        };

        let position_quantity = self
            .managed_position()
            .map(|managed| managed.position.quantity.as_f64());
        let Some(facts) = exit_pre_submit_rejection_facts(decision, position_quantity)? else {
            return Ok(());
        };
        let decision_trace_id = self.active_decision_trace_id();
        let ts = unix_nanos_from_millis(now_ms)?;
        evidence.write_exit_pre_submit_rejection(&decision_trace_id, facts, ts, ts)
    }

    fn exit_evaluation_decision_facts(
        &self,
        decision: &ExitSubmissionDecision,
    ) -> Result<Option<BoltV3ExitEvaluationFacts>> {
        if decision.blocked_reason.is_some() {
            return Ok(None);
        }

        let Some(exit_decision) = decision.evaluation.exit_decision else {
            return Ok(None);
        };
        let Some(exit_decision_reason) =
            exit_decision_reason_as_decision_fact(exit_decision, &decision.forced_flat_reasons)
        else {
            return Ok(None);
        };

        let position_quantity = self
            .managed_position()
            .map(|managed| managed.position.quantity.as_f64())
            .ok_or_else(|| anyhow::anyhow!("exit evaluation event requires managed position"))?;
        let exit_quantity = decision
            .quantity
            .map(|quantity| quantity.as_f64())
            .ok_or_else(|| anyhow::anyhow!("exit evaluation event requires exit quantity"))?;
        let open_exit_order_quantity = 0.0;
        let uncovered_position_quantity = (position_quantity - open_exit_order_quantity).max(0.0);

        Ok(Some(BoltV3ExitEvaluationFacts {
            authoritative_position_quantity: Some(position_quantity),
            authoritative_sellable_quantity: Some(exit_quantity),
            open_exit_order_quantity: Some(open_exit_order_quantity),
            uncovered_position_quantity: Some(uncovered_position_quantity),
            exit_order_mechanical_outcome: "accepted".to_string(),
            exit_order_mechanical_rejection_reason: None,
            exit_decision: "exit".to_string(),
            exit_decision_reason: exit_decision_reason.to_string(),
            archetype_metrics: json!({
                "hold_ev_bps": decision.evaluation.hold_ev_bps,
                "exit_ev_bps": decision.evaluation.exit_ev_bps,
                "forced_flat_reasons": forced_flat_reasons_as_decision_facts(&decision.forced_flat_reasons),
            }),
        }))
    }

    fn try_submit_exit_order(&mut self, now_ms: u64) -> Result<Option<ClientOrderId>> {
        let mut decision = self.exit_submission_decision_at(now_ms);
        self.write_bolt_v3_exit_pre_submit_rejection(now_ms, &decision)?;

        let Some(instrument_id) = decision.instrument_id else {
            self.log_exit_evaluation(now_ms, &decision);
            return Ok(None);
        };
        let Some(order_side) = decision.order_side else {
            self.log_exit_evaluation(now_ms, &decision);
            return Ok(None);
        };
        let Some(raw_price) = decision.price else {
            self.log_exit_evaluation(now_ms, &decision);
            return Ok(None);
        };
        let Some(quantity) = decision.quantity else {
            self.log_exit_evaluation(now_ms, &decision);
            return Ok(None);
        };
        let instrument = self
            .current_instrument(instrument_id)
            .ok_or_else(|| anyhow::anyhow!("exit instrument missing from cache"))?;
        let price = Price::new(raw_price, instrument.price_precision());
        let client_order_id = self.core.order_factory().generate_client_order_id();
        decision.client_order_id = Some(client_order_id);
        self.log_exit_evaluation(now_ms, &decision);
        let order_type = OrderType::Limit;
        let time_in_force = TimeInForce::Fok;
        let order = self.core.order_factory().limit(
            instrument_id,
            order_side,
            quantity,
            price,
            Some(time_in_force),
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
            Some(client_order_id),
        );
        self.write_bolt_v3_exit_evaluation(now_ms, &decision)?;

        let client_id = ClientId::from(self.config.client_id.as_str());
        let Some(managed_position) = self.managed_position().cloned() else {
            anyhow::bail!("exit submit requires managed position state");
        };
        self.exposure = ExposureState::ExitPending(ExitPendingState {
            position: Some(managed_position.clone()),
            pending_exit: PendingExitState {
                client_order_id,
                market_id: managed_position.position.market_id.clone(),
                position_id: Some(managed_position.position.position_id),
                fill_received: false,
                close_received: false,
            },
        });
        log::info!(
            "eth_chainlink_taker exit submit: strategy_id={} instrument_id={} order_side={:?} price={} quantity={} client_order_id={}",
            self.config.strategy_id,
            instrument_id,
            order_side,
            price,
            quantity,
            client_order_id,
        );

        let evidence = self.context.bolt_v3_decision_evidence.clone();
        let submit_result = if let Some(evidence) = evidence {
            let decision_trace_id = self.active_decision_trace_id();
            let facts = order_submission_facts(
                order_type,
                time_in_force,
                instrument_id,
                order_side,
                price,
                quantity,
                false,
                client_order_id,
            )?;
            let ts = unix_nanos_from_millis(now_ms)?;
            evidence.gate_exit_order_submission(&decision_trace_id, facts, ts, ts, || {
                self.submit_order(order, None, Some(client_id))
            })
        } else {
            self.submit_order(order, None, Some(client_id))
        };

        if let Err(error) = submit_result {
            self.exposure = ExposureState::Managed(managed_position);
            return Err(error);
        }

        Ok(Some(client_order_id))
    }

    fn entry_submission_decision_at(&self, now_ms: u64) -> EntrySubmissionDecision {
        let evaluation = self.entry_evaluation_at(now_ms);
        let mut decision = EntrySubmissionDecision {
            evaluation: evaluation.clone(),
            instrument_id: self.active.instrument_id,
            order_side: None,
            price: None,
            quantity_value: None,
            client_order_id: None,
            blocked_reason: None,
        };

        if self.core.trader_id().is_none() {
            decision.blocked_reason = Some("strategy_core_not_registered");
            return decision;
        }

        if !evaluation.gate.blocked_by.is_empty() {
            decision.blocked_reason = Some("entry_gate_blocked");
            return decision;
        }
        if !evaluation.pricing_blocked_by.is_empty() {
            decision.blocked_reason = Some("entry_pricing_blocked");
            return decision;
        }

        let Some(selected_side) = evaluation.selected_side else {
            decision.blocked_reason = Some("no_side_selected");
            return decision;
        };
        let Some(sized_notional_usdc) = evaluation
            .sized_notional_usdc
            .filter(|value| value.is_finite() && *value > 0.0)
        else {
            decision.blocked_reason =
                if evaluation.selected_side.is_some() && self.config.max_position_usdc <= 0.0 {
                    Some("position_limit_reached")
                } else {
                    Some("sized_notional_not_positive")
                };
            return decision;
        };

        let Some(instrument_id) = self.instrument_id_for_side(selected_side) else {
            decision.blocked_reason = Some("instrument_id_missing");
            return decision;
        };
        let order_side = strategy_entry_order_side(selected_side);
        decision.instrument_id = Some(instrument_id);
        decision.order_side = Some(order_side);

        let Some(instrument) = self.current_instrument(instrument_id) else {
            decision.blocked_reason = Some("instrument_missing_from_cache");
            return decision;
        };
        let Some(price) = self.submission_entry_price(selected_side) else {
            decision.blocked_reason = Some("entry_price_missing");
            return decision;
        };
        let Some(entry_cost) = self.executable_entry_cost(selected_side) else {
            decision.blocked_reason = Some("entry_cost_missing");
            return decision;
        };
        let shares_value = sized_notional_usdc / entry_cost;
        let Ok(quantity) = instrument.try_make_qty(shares_value, Some(true)) else {
            decision.blocked_reason = Some("quantity_rounding_failed");
            return decision;
        };
        let quantity_value = quantity.as_f64();
        if !quantity_value.is_finite() || quantity_value <= 0.0 {
            decision.blocked_reason = Some("quantity_not_positive");
            return decision;
        }

        decision.price = Some(price);
        decision.quantity_value = Some(quantity_value);
        decision
    }

    fn try_submit_entry_order(&mut self, now_ms: u64) -> Result<Option<ClientOrderId>> {
        let decision = self.entry_submission_decision_at(now_ms);
        self.write_bolt_v3_entry_evaluation(now_ms, &decision)?;
        self.write_bolt_v3_entry_pre_submit_rejection(now_ms, &decision)?;
        self.log_entry_evaluation(now_ms, &decision);

        let Some(instrument_id) = decision.instrument_id else {
            if let Some(reason) = decision.blocked_reason {
                log::warn!(
                    "eth_chainlink_taker entry submit skipped: strategy_id={} reason={}",
                    self.config.strategy_id,
                    reason
                );
            }
            return Ok(None);
        };
        let Some(order_side) = decision.order_side else {
            if let Some(reason) = decision.blocked_reason {
                log::warn!(
                    "eth_chainlink_taker entry submit skipped: strategy_id={} reason={}",
                    self.config.strategy_id,
                    reason
                );
            }
            return Ok(None);
        };
        let Some(price) = decision.price else {
            if let Some(reason) = decision.blocked_reason {
                log::warn!(
                    "eth_chainlink_taker entry submit skipped: strategy_id={} reason={}",
                    self.config.strategy_id,
                    reason
                );
            }
            return Ok(None);
        };
        let Some(quantity_value) = decision.quantity_value else {
            if let Some(reason) = decision.blocked_reason {
                log::warn!(
                    "eth_chainlink_taker entry submit skipped: strategy_id={} reason={}",
                    self.config.strategy_id,
                    reason
                );
            }
            return Ok(None);
        };
        let Some(historical_entry_fee_bps) = decision
            .evaluation
            .selected_side
            .and_then(|selected_side| self.outcome_fee_bps(selected_side))
        else {
            log::warn!(
                "eth_chainlink_taker entry submit skipped: strategy_id={} reason=historical_entry_fee_unavailable",
                self.config.strategy_id
            );
            return Ok(None);
        };
        let instrument = self
            .current_instrument(instrument_id)
            .ok_or_else(|| anyhow::anyhow!("entry instrument missing from cache"))?;
        let quantity = instrument.try_make_qty(quantity_value, Some(true))?;

        if self.exposure_occupancy().is_some() {
            let _ = self.enforce_one_position_invariant();
            return Ok(None);
        }

        let price = Price::new(price, instrument.price_precision());
        let client_order_id = self.core.order_factory().generate_client_order_id();
        let order_type = OrderType::Limit;
        let time_in_force = TimeInForce::Fok;
        let order = self.core.order_factory().limit(
            instrument_id,
            order_side,
            quantity,
            price,
            Some(time_in_force),
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
            Some(client_order_id),
        );

        let client_id = ClientId::from(self.config.client_id.as_str());
        self.exposure = ExposureState::PendingEntry(PendingEntryState {
            client_order_id,
            market_id: self.current_market_id().map(str::to_string),
            instrument_id,
            outcome_side: decision.evaluation.selected_side,
            outcome_fees: self.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(historical_entry_fee_bps),
            interval_open: self.active.interval_open,
            selection_published_at_ms: self.active.selection_published_at_ms,
            seconds_to_expiry_at_selection: self.active.seconds_to_expiry_at_selection,
            book: match decision.evaluation.selected_side {
                Some(OutcomeSide::Up)
                    if self.active.books.up.instrument_id == Some(instrument_id) =>
                {
                    self.active.books.up.clone()
                }
                Some(OutcomeSide::Down)
                    if self.active.books.down.instrument_id == Some(instrument_id) =>
                {
                    self.active.books.down.clone()
                }
                _ => OutcomeBookState::from_instrument_id(instrument_id),
            },
        });
        log::info!(
            "eth_chainlink_taker entry submit: strategy_id={} instrument_id={} order_side={:?} price={} quantity={} client_order_id={}",
            self.config.strategy_id,
            instrument_id,
            order_side,
            price,
            quantity,
            client_order_id,
        );

        let evidence = self.context.bolt_v3_decision_evidence.clone();
        let submit_result = if let Some(evidence) = evidence {
            let decision_trace_id = self.active_decision_trace_id();
            let facts = order_submission_facts(
                order_type,
                time_in_force,
                instrument_id,
                order_side,
                price,
                quantity,
                false,
                client_order_id,
            )?;
            let ts = unix_nanos_from_millis(now_ms)?;
            evidence.gate_entry_order_submission(&decision_trace_id, facts, ts, ts, || {
                self.submit_order(order, None, Some(client_id))
            })
        } else {
            self.submit_order(order, None, Some(client_id))
        };

        if let Err(error) = submit_result {
            self.clear_pending_entry_state();
            return Err(error);
        }

        Ok(Some(client_order_id))
    }

    fn entry_evaluation_at(&self, now_ms: u64) -> EntryEvaluation {
        let gate = self.entry_gate_decision_at(now_ms);
        let mut evaluation = EntryEvaluation {
            gate,
            pricing_blocked_by: Vec::new(),
            fair_probability_up: None,
            uncertainty_band_probability: None,
            up_worst_case_ev_bps: None,
            down_worst_case_ev_bps: None,
            min_worst_case_ev_bps: None,
            expected_ev_per_usdc: None,
            book_impact_cap_usdc: None,
            sized_notional_usdc: None,
            selected_side: None,
        };

        if !evaluation.gate.blocked_by.is_empty() {
            return evaluation;
        }

        let pricing_inputs = match self.current_entry_pricing_inputs_at(now_ms) {
            Ok(inputs) => inputs,
            Err(blocked_by) => {
                evaluation.pricing_blocked_by = blocked_by;
                return evaluation;
            }
        };
        evaluation.min_worst_case_ev_bps = Some(pricing_inputs.theta_scaled_min_edge_bps);

        let fair_probability_up = match self.current_fair_probability_up_at(now_ms) {
            Some(value) => value,
            None => {
                evaluation
                    .pricing_blocked_by
                    .push(EntryPricingBlockReason::FairProbabilityUnavailable);
                return evaluation;
            }
        };
        evaluation.fair_probability_up = Some(fair_probability_up);

        let up_fee_bps = match self.outcome_fee_bps(OutcomeSide::Up) {
            Some(value) => value,
            None => {
                evaluation
                    .pricing_blocked_by
                    .push(EntryPricingBlockReason::FeeUnavailable(OutcomeSide::Up));
                return evaluation;
            }
        };
        let down_fee_bps = match self.outcome_fee_bps(OutcomeSide::Down) {
            Some(value) => value,
            None => {
                evaluation
                    .pricing_blocked_by
                    .push(EntryPricingBlockReason::FeeUnavailable(OutcomeSide::Down));
                return evaluation;
            }
        };
        let uncertainty_band_probability =
            match self.current_uncertainty_band_probability_at(now_ms, up_fee_bps, down_fee_bps) {
                Some(value) => value,
                None => {
                    evaluation
                        .pricing_blocked_by
                        .push(EntryPricingBlockReason::UncertaintyBandUnavailable);
                    return evaluation;
                }
            };
        evaluation.uncertainty_band_probability = Some(uncertainty_band_probability);
        let up_entry_cost = match self.executable_entry_cost(OutcomeSide::Up) {
            Some(value) => value,
            None => {
                evaluation.pricing_blocked_by.push(
                    EntryPricingBlockReason::ExecutableEntryCostUnavailable(OutcomeSide::Up),
                );
                return evaluation;
            }
        };
        let down_entry_cost = match self.executable_entry_cost(OutcomeSide::Down) {
            Some(value) => value,
            None => {
                evaluation.pricing_blocked_by.push(
                    EntryPricingBlockReason::ExecutableEntryCostUnavailable(OutcomeSide::Down),
                );
                return evaluation;
            }
        };

        evaluation.up_worst_case_ev_bps = compute_worst_case_ev_bps(
            OutcomeSide::Up,
            &WorstCaseEvInputs {
                fair_probability: Some(fair_probability_up),
                uncertainty_band_probability,
                executable_entry_cost: up_entry_cost,
                fee_bps: Some(up_fee_bps),
            },
        );
        if evaluation.up_worst_case_ev_bps.is_none() {
            evaluation
                .pricing_blocked_by
                .push(EntryPricingBlockReason::WorstCaseEvUnavailable(
                    OutcomeSide::Up,
                ));
        }

        evaluation.down_worst_case_ev_bps = compute_worst_case_ev_bps(
            OutcomeSide::Down,
            &WorstCaseEvInputs {
                fair_probability: Some(fair_probability_up),
                uncertainty_band_probability,
                executable_entry_cost: down_entry_cost,
                fee_bps: Some(down_fee_bps),
            },
        );
        if evaluation.down_worst_case_ev_bps.is_none() {
            evaluation
                .pricing_blocked_by
                .push(EntryPricingBlockReason::WorstCaseEvUnavailable(
                    OutcomeSide::Down,
                ));
        }

        if !evaluation.pricing_blocked_by.is_empty() {
            return evaluation;
        }

        evaluation.selected_side = choose_entry_side(&SideSelectionInputs {
            up_worst_ev_bps: evaluation.up_worst_case_ev_bps,
            down_worst_ev_bps: evaluation.down_worst_case_ev_bps,
            min_worst_case_ev_bps: pricing_inputs.theta_scaled_min_edge_bps,
        });
        if let Some(selected_side) = evaluation.selected_side {
            let selected_worst_case_ev_bps = match selected_side {
                OutcomeSide::Up => evaluation.up_worst_case_ev_bps,
                OutcomeSide::Down => evaluation.down_worst_case_ev_bps,
            };
            let expected_ev_per_usdc =
                selected_worst_case_ev_bps.map(|ev_bps| ev_bps / BPS_DENOMINATOR);
            let book_impact_cap_usdc = self.visible_book_notional_cap_usdc(selected_side);
            evaluation.expected_ev_per_usdc = expected_ev_per_usdc;
            evaluation.book_impact_cap_usdc = book_impact_cap_usdc;
            if let (Some(expected_ev_per_usdc), Some(book_impact_cap_usdc)) =
                (expected_ev_per_usdc, book_impact_cap_usdc)
            {
                evaluation.sized_notional_usdc = Some(choose_robust_size(&RobustSizingInputs {
                    expected_ev_per_usdc,
                    risk_lambda: self.config.risk_lambda,
                    max_position_usdc: self.config.max_position_usdc,
                    impact_cap_usdc: book_impact_cap_usdc,
                }));
            }
        }
        evaluation
    }
}

fn entry_no_action_reason(decision: &EntrySubmissionDecision) -> Option<&'static str> {
    if decision.blocked_reason == Some("no_side_selected")
        && decision.evaluation.gate.blocked_by.is_empty()
        && decision.evaluation.pricing_blocked_by.is_empty()
    {
        return Some("insufficient_edge");
    }

    if decision.blocked_reason == Some("entry_gate_blocked")
        && !decision.evaluation.gate.blocked_by.is_empty()
        && decision.evaluation.gate.blocked_by.iter().all(|reason| {
            matches!(
                reason,
                EntryBlockReason::IntervalOpenMissing | EntryBlockReason::WarmupIncomplete
            )
        })
    {
        return Some("missing_reference_quote");
    }

    if decision.blocked_reason == Some("entry_gate_blocked")
        && decision.evaluation.gate.blocked_by == [EntryBlockReason::FeesNotReady]
    {
        return Some("fee_rate_unavailable");
    }

    if decision.blocked_reason == Some("entry_gate_blocked")
        && decision.evaluation.gate.blocked_by
            == [EntryBlockReason::ForcedFlat(
                ForcedFlatReason::StaleChainlink,
            )]
    {
        return Some("stale_reference_quote");
    }

    if decision.blocked_reason == Some("entry_pricing_blocked")
        && decision.evaluation.pricing_blocked_by
            == [EntryPricingBlockReason::FairProbabilityUnavailable]
    {
        return Some("fair_probability_unavailable");
    }

    if decision.blocked_reason == Some("position_limit_reached")
        && decision
            .evaluation
            .sized_notional_usdc
            .is_some_and(|value| value <= 0.0)
    {
        return Some("position_limit_reached");
    }

    None
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
        self.bootstrap_recovery_from_cache();
        self.register_shell_subscriptions();
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        self.deregister_shell_subscriptions();
        Ok(())
    }

    fn on_book_deltas(
        &mut self,
        deltas: &nautilus_model::data::OrderBookDeltas,
    ) -> anyhow::Result<()> {
        let mut matched = self.active.books.update_from_deltas(deltas);
        self.sync_exposure_context_from_active();
        if self
            .tracked_observed_position()
            .is_some_and(|position| position.instrument_id == deltas.instrument_id)
            && !(self.active.books.up.instrument_id == Some(deltas.instrument_id)
                || self.active.books.down.instrument_id == Some(deltas.instrument_id))
        {
            if let Some(open_position) = self.tracked_observed_position_mut() {
                open_position.book.update_from_deltas(deltas);
            }
            matched = true;
        }
        if self
            .pending_entry()
            .is_some_and(|pending| pending.instrument_id == deltas.instrument_id)
            && !(self.active.books.up.instrument_id == Some(deltas.instrument_id)
                || self.active.books.down.instrument_id == Some(deltas.instrument_id))
        {
            if let Some(pending) = self.pending_entry_mut() {
                pending.book.update_from_deltas(deltas);
            }
            matched = true;
        }

        if !matched {
            return Ok(());
        }

        let now_ms = self.clock().timestamp_ns().as_u64() / 1_000_000;
        if matches!(self.exposure, ExposureState::Managed(_)) {
            let _ = self.try_submit_exit_order(now_ms)?;
        }
        let _ = self.try_submit_entry_order(now_ms)?;
        Ok(())
    }

    fn on_order_filled(
        &mut self,
        event: &nautilus_model::events::OrderFilled,
    ) -> anyhow::Result<()> {
        let now_ms = event.ts_event.as_u64() / 1_000_000;
        let entry_fill = self
            .pending_entry()
            .is_some_and(|pending| pending.client_order_id == event.client_order_id);
        let exit_fill = self
            .exposure
            .exit_pending()
            .is_some_and(|exit| exit.pending_exit.client_order_id == event.client_order_id);

        if entry_fill {
            let pending_context = self.pending_entry_context_for(event.instrument_id);
            let position_side = infer_strategy_position_side_from_entry_fill(event.order_side);
            if let (Some(position_id), Some(position_side)) = (event.position_id, position_side) {
                self.exposure = ExposureState::Managed(ManagedPositionState {
                    position: self.build_open_position_state(
                        None,
                        pending_context.as_ref(),
                        PositionMaterializationSpec {
                            instrument_id: event.instrument_id,
                            position_id,
                            entry_order_side: event.order_side,
                            side: position_side,
                            quantity: event.last_qty,
                            avg_px_open: event.last_px.as_f64(),
                        },
                        true,
                    ),
                    origin: ManagedPositionOrigin::StrategyEntry,
                });
                self.sync_exposure_context_from_active();
                self.refresh_book_subscriptions_for_current_state();
            } else {
                if let Some(pending) = pending_context.clone() {
                    let reason = if event.position_id.is_none() {
                        EntryReconcileReason::AwaitingPositionMaterialization
                    } else {
                        EntryReconcileReason::UnsupportedEntryFillSide {
                            order_side: event.order_side,
                        }
                    };
                    self.exposure = ExposureState::EntryReconcilePending { pending, reason };
                } else {
                    self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
                        reason: BlindRecoveryReason::InvalidLivePosition {
                            entry_order_side: event.order_side,
                            side: position_side.unwrap_or(PositionSide::Flat),
                        },
                    });
                }
                log::error!(
                    "eth_chainlink_taker entry fill could not materialize supported long position: strategy_id={} client_order_id={} instrument_id={} order_side={:?} position_id_present={} position_side_inferable={}",
                    self.config.strategy_id,
                    event.client_order_id,
                    event.instrument_id,
                    event.order_side,
                    event.position_id.is_some(),
                    position_side.is_some(),
                );
            }
            if let Some(market_id) = pending_context.and_then(|pending| pending.market_id.clone()) {
                self.record_market_fill(&market_id, now_ms);
            }
        } else if exit_fill {
            if let Some(market_id) = self
                .exposure
                .exit_pending()
                .and_then(|exit| exit.pending_exit.market_id.clone())
                .or_else(|| self.current_position_market_id())
            {
                self.record_market_fill(&market_id, now_ms);
            }
            if let Some(exit_pending) = self.exposure.exit_pending_mut() {
                exit_pending.pending_exit.fill_received = true;
                if exit_pending.pending_exit.close_received {
                    self.exposure = ExposureState::Flat;
                }
            }
        }
        self.prune_market_lifecycle(now_ms);
        Ok(())
    }

    fn on_order_canceled(
        &mut self,
        event: &nautilus_model::events::OrderCanceled,
    ) -> anyhow::Result<()> {
        if matches!(
            &self.exposure,
            ExposureState::PendingEntry(pending) if pending.client_order_id == event.client_order_id
        ) {
            self.clear_pending_entry_state();
        }
        if let Some(exit_pending) = self.exposure.exit_pending().cloned()
            && exit_pending.pending_exit.client_order_id == event.client_order_id
            && !exit_pending.pending_exit.fill_received
        {
            self.exposure = match exit_pending.position {
                Some(position) => ExposureState::Managed(position),
                None => ExposureState::Flat,
            };
        }
        self.prune_market_lifecycle(event.ts_event.as_u64() / 1_000_000);
        Ok(())
    }
}

nautilus_strategy!(EthChainlinkTaker, {
    fn on_order_rejected(&mut self, event: nautilus_model::events::OrderRejected) {
        if matches!(
            &self.exposure,
            ExposureState::PendingEntry(pending) if pending.client_order_id == event.client_order_id
        ) {
            self.clear_pending_entry_state();
        }
        if let Some(exit_pending) = self.exposure.exit_pending().cloned()
            && exit_pending.pending_exit.client_order_id == event.client_order_id
            && !exit_pending.pending_exit.fill_received
        {
            self.exposure = match exit_pending.position {
                Some(position) => ExposureState::Managed(position),
                None => ExposureState::Flat,
            };
        }
        self.prune_market_lifecycle(event.ts_event.as_u64() / 1_000_000);
    }

    fn on_order_expired(&mut self, event: nautilus_model::events::OrderExpired) {
        if matches!(
            &self.exposure,
            ExposureState::PendingEntry(pending) if pending.client_order_id == event.client_order_id
        ) {
            self.clear_pending_entry_state();
        }
        if let Some(exit_pending) = self.exposure.exit_pending().cloned()
            && exit_pending.pending_exit.client_order_id == event.client_order_id
            && !exit_pending.pending_exit.fill_received
        {
            self.exposure = match exit_pending.position {
                Some(position) => ExposureState::Managed(position),
                None => ExposureState::Flat,
            };
        }
        self.prune_market_lifecycle(event.ts_event.as_u64() / 1_000_000);
    }

    fn on_position_opened(&mut self, _event: nautilus_model::events::PositionOpened) {
        self.materialize_position_from_event(
            _event.instrument_id,
            _event.position_id,
            _event.entry,
            _event.side,
            _event.quantity,
            _event.avg_px_open,
        );
    }

    fn on_position_changed(&mut self, _event: nautilus_model::events::PositionChanged) {
        self.materialize_position_from_event(
            _event.instrument_id,
            _event.position_id,
            _event.entry,
            _event.side,
            _event.quantity,
            _event.avg_px_open,
        );
    }

    fn on_position_closed(&mut self, _event: nautilus_model::events::PositionClosed) {
        match &mut self.exposure {
            ExposureState::Managed(position)
                if position.position.position_id == _event.position_id =>
            {
                self.exposure = ExposureState::Flat;
            }
            ExposureState::ExitPending(exit_pending)
                if exit_pending.pending_exit.position_id == Some(_event.position_id) =>
            {
                exit_pending.pending_exit.close_received = true;
                exit_pending.position = None;
                if exit_pending.is_terminal() {
                    self.exposure = ExposureState::Flat;
                }
            }
            ExposureState::UnsupportedObserved(observed)
                if observed.observed.position_id == _event.position_id =>
            {
                self.exposure = ExposureState::Flat;
            }
            _ => {}
        }
        self.refresh_book_subscriptions_for_current_state();
        self.prune_market_lifecycle(_event.ts_event.as_u64() / 1_000_000);
    }
});

#[derive(Debug)]
pub struct EthChainlinkTakerBuilder;

pub const ETH_CHAINLINK_TAKER_KIND: &str = "eth_chainlink_taker";

impl EthChainlinkTakerBuilder {
    fn parse_config(raw: &Value) -> Result<EthChainlinkTakerConfig> {
        raw.clone()
            .try_into()
            .context("eth_chainlink_taker builder requires a valid config table")
    }

    pub fn build_concrete(
        raw: &Value,
        context: &StrategyBuildContext,
    ) -> Result<EthChainlinkTaker> {
        Ok(EthChainlinkTaker::new(
            Self::parse_config(raw)?,
            context.clone(),
        ))
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
        ETH_CHAINLINK_TAKER_KIND
    }

    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
        let Some(table) = raw.as_table() else {
            Self::push_wrong_type(errors, field_prefix.to_string(), "a table", raw);
            return;
        };

        Self::validate_table(table, field_prefix, errors);
    }

    fn build(raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        Ok(Box::new(Self::build_concrete(raw, context)?))
    }

    fn register(
        raw: &Value,
        context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId> {
        let strategy = Self::build_concrete(raw, context)?;
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
    let previous_books = active.books.clone();
    let next = ActiveMarketState::from_snapshot(snapshot, warmup_target);
    let preserve_books = active.market_id.is_some()
        && active.market_id == next.market_id
        && active.instrument_id == next.instrument_id;
    if active.same_boundary(&next) {
        return;
    }
    if same_market_transition(active, &next) {
        active.phase = next.phase;
        active.forced_flat = next.forced_flat;
        return;
    }
    *active = next;
    if preserve_books {
        active.books = previous_books;
    }
}

fn same_market_transition(current: &ActiveMarketState, next: &ActiveMarketState) -> bool {
    current.market_id.is_some()
        && current.market_id == next.market_id
        && current.instrument_id == next.instrument_id
        && current.interval_start_ms == next.interval_start_ms
}

fn same_market_interval_rollover(current: &ActiveMarketState, next: &ActiveMarketState) -> bool {
    current.market_id.is_some()
        && current.market_id == next.market_id
        && current.instrument_id == next.instrument_id
        && current.interval_start_ms != next.interval_start_ms
}

fn selection_book_subscriptions(snapshot: &RuntimeSelectionSnapshot) -> OutcomeBookSubscriptions {
    match &snapshot.decision.state {
        SelectionState::Active { market } | SelectionState::Freeze { market, .. } => {
            OutcomeBookSubscriptions::from_market(market)
        }
        SelectionState::Idle { .. } => OutcomeBookSubscriptions::default(),
    }
}

fn market_selection_result_facts(
    snapshot: &RuntimeSelectionSnapshot,
    target_context: &BoltV3MarketSelectionContext,
) -> Result<Option<BoltV3MarketSelectionResultFacts>> {
    let (market, market_selection_outcome, market_selection_failure_reason) =
        match &snapshot.decision.state {
            SelectionState::Active { market } | SelectionState::Freeze { market, .. } => (
                Some(market),
                market_selection_outcome(market, snapshot.published_at_ms)?,
                None,
            ),
            SelectionState::Idle { reason } => (
                None,
                "failed".to_string(),
                Some(market_selection_failure_reason(reason)?),
            ),
        };

    let up_instrument_id = market.map(|market| {
        polymarket_instrument_id(&market.condition_id, &market.up_token_id).to_string()
    });
    let down_instrument_id = market.map(|market| {
        polymarket_instrument_id(&market.condition_id, &market.down_token_id).to_string()
    });

    Ok(Some(BoltV3MarketSelectionResultFacts {
        market_selection_type: target_context.market_selection_type.clone(),
        market_selection_timestamp_milliseconds: snapshot.published_at_ms,
        market_selection_outcome,
        market_selection_failure_reason,
        rotating_market_family: target_context.rotating_market_family.clone(),
        underlying_asset: target_context.underlying_asset.clone(),
        cadence_seconds: target_context.cadence_seconds,
        market_selection_rule: target_context.market_selection_rule.clone(),
        retry_interval_seconds: target_context.retry_interval_seconds,
        blocked_after_seconds: target_context.blocked_after_seconds,
        polymarket_condition_id: market.map(|market| market.condition_id.clone()),
        polymarket_market_slug: market.map(|market| market.market_slug.clone()),
        polymarket_question_id: market.map(|market| market.question_id.clone()),
        up_instrument_id,
        down_instrument_id,
        selected_market_observed_timestamp: market
            .map(|market| market.selected_market_observed_ts_ms),
        polymarket_market_start_timestamp_milliseconds: market.map(|market| market.start_ts_ms),
        polymarket_market_end_timestamp_milliseconds: market.map(|market| market.end_ts_ms),
        price_to_beat_value: market.and_then(|market| market.price_to_beat),
        price_to_beat_observed_timestamp: market
            .and_then(|market| market.price_to_beat_observed_ts_ms),
        price_to_beat_source: market.and_then(|market| market.price_to_beat_source.clone()),
    }))
}

fn market_selection_failure_reason(reason: &str) -> Result<String> {
    validate_bolt_v3_market_selection_failure_reason(reason)?;
    Ok(reason.to_string())
}

fn market_selection_outcome(market: &CandidateMarket, timestamp_ms: u64) -> Result<String> {
    if market.start_ts_ms <= timestamp_ms && timestamp_ms < market.end_ts_ms {
        return Ok("current".to_string());
    }
    if market.start_ts_ms > timestamp_ms {
        return Ok("next".to_string());
    }
    Err(anyhow!(
        "selected market {} ended before market_selection_timestamp_milliseconds {}",
        market.market_id,
        timestamp_ms
    ))
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
    if let Some(instrument_id) = current.tracked_position_instrument_id
        && next.tracked_position_instrument_id != Some(instrument_id)
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
    if let Some(instrument_id) = next.tracked_position_instrument_id
        && current.tracked_position_instrument_id != Some(instrument_id)
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

const BPS_DENOMINATOR: f64 = 10_000.0;
const QUADRATIC_RISK_DIVISOR: f64 = 2.0;
const MILLIS_PER_SECOND_U64: u64 = 1_000;
const MILLIS_PER_SECOND_F64: f64 = 1_000.0;
const SECONDS_PER_YEAR_F64: f64 = 365.25 * 24.0 * 60.0 * 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutcomeSide {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq)]
struct LeadVenueSignal {
    venue_name: String,
    price: Option<f64>,
    observed_ts_ms: Option<u64>,
    age_ms: u64,
    jitter_ms: u64,
    agreement_corr: f64,
    effective_weight: f64,
    lead_gap_probability: f64,
}

impl LeadVenueSignal {
    fn is_eligible(&self, min_agreement_corr: f64, max_jitter_ms: u64) -> bool {
        self.agreement_corr.is_finite()
            && self.agreement_corr >= min_agreement_corr
            && self.jitter_ms <= max_jitter_ms
            && self.effective_weight.is_finite()
            && self.effective_weight > 0.0
            && sanitize_probability(self.lead_gap_probability).is_some()
    }
}

fn arbitrate_lead_reference(
    candidates: &[LeadVenueSignal],
    min_agreement_corr: f64,
    max_jitter_ms: u64,
) -> Option<&LeadVenueSignal> {
    let mut ranked = candidates
        .iter()
        .filter_map(|candidate| {
            lead_composite_score(candidate, min_agreement_corr, max_jitter_ms)
                .map(|score| (candidate, score))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(_, left_score), (_, right_score)| right_score.total_cmp(left_score));

    let (best_candidate, best_score) = ranked.first().copied()?;
    if ranked
        .get(1)
        .is_some_and(|(_, second_score)| second_score == &best_score)
    {
        return None;
    }

    Some(best_candidate)
}

fn lead_composite_score(
    candidate: &LeadVenueSignal,
    min_agreement_corr: f64,
    max_jitter_ms: u64,
) -> Option<f64> {
    if !candidate.is_eligible(min_agreement_corr, max_jitter_ms) {
        return None;
    }

    let freshness_score = 1.0 / (candidate.age_ms as f64 + 1.0);
    let jitter_score = 1.0 / (candidate.jitter_ms as f64 + 1.0);

    Some(candidate.agreement_corr + freshness_score + jitter_score)
}

fn best_healthy_oracle_price(snapshot: &ReferenceSnapshot) -> Option<f64> {
    snapshot
        .venues
        .iter()
        .filter(|venue| {
            venue.venue_kind == crate::platform::reference::VenueKind::Oracle
                && !venue.stale
                && matches!(
                    venue.health,
                    crate::platform::reference::VenueHealth::Healthy
                )
                && venue.effective_weight.is_finite()
                && venue.effective_weight > 0.0
                && venue
                    .observed_price
                    .is_some_and(|price| price.is_finite() && price > 0.0)
        })
        .max_by(|lhs, rhs| {
            lhs.effective_weight
                .total_cmp(&rhs.effective_weight)
                .then_with(|| lhs.observed_ts_ms.cmp(&rhs.observed_ts_ms))
                .then_with(|| lhs.venue_name.cmp(&rhs.venue_name))
        })
        .and_then(|venue| venue.observed_price)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct UncertaintyBandInputs {
    lead_gap_probability: f64,
    jitter_penalty_probability: f64,
    time_uncertainty_probability: f64,
    fee_uncertainty_probability: f64,
}

fn uncertainty_band_probability(inputs: &UncertaintyBandInputs) -> Option<f64> {
    sanitize_probability(
        sanitize_probability(inputs.lead_gap_probability)?
            + sanitize_probability(inputs.jitter_penalty_probability)?
            + sanitize_probability(inputs.time_uncertainty_probability)?
            + sanitize_probability(inputs.fee_uncertainty_probability)?,
    )
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct WorstCaseEvInputs {
    fair_probability: Option<f64>,
    uncertainty_band_probability: f64,
    executable_entry_cost: f64,
    fee_bps: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct FairProbabilityInputs {
    spot_price: f64,
    strike_price: f64,
    seconds_to_expiry: u64,
    realized_vol: f64,
    pricing_kurtosis: f64,
}

fn compute_fair_probability_up(inputs: &FairProbabilityInputs) -> Option<f64> {
    if !inputs.spot_price.is_finite()
        || inputs.spot_price <= 0.0
        || !inputs.strike_price.is_finite()
        || inputs.strike_price <= 0.0
        || !inputs.realized_vol.is_finite()
        || inputs.realized_vol <= 0.0
        || !inputs.pricing_kurtosis.is_finite()
    {
        return None;
    }

    let sigma_eff = inputs.realized_vol * (1.0 + inputs.pricing_kurtosis / 6.0);
    if !sigma_eff.is_finite() || sigma_eff <= 0.0 {
        return None;
    }

    let time_to_expiry_years = inputs.seconds_to_expiry as f64 / SECONDS_PER_YEAR_F64;
    if time_to_expiry_years <= 0.0 {
        return Some(match inputs.spot_price.total_cmp(&inputs.strike_price) {
            std::cmp::Ordering::Greater => 1.0,
            std::cmp::Ordering::Less => 0.0,
            std::cmp::Ordering::Equal => 0.5,
        });
    }

    let d2 = ((inputs.spot_price / inputs.strike_price).ln()
        - (sigma_eff.powi(2) / 2.0) * time_to_expiry_years)
        / (sigma_eff * time_to_expiry_years.sqrt());
    sanitize_probability(standard_normal_cdf(d2))
}

fn standard_normal_cdf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.231_641_9 * x.abs());
    let d = 0.398_942_3 * (-x * x / 2.0).exp();
    let prob = d
        * t
        * (0.319_381_5 + t * (-0.356_563_8 + t * (1.781_478 + t * (-1.821_256 + t * 1.330_274))));
    if x > 0.0 { 1.0 - prob } else { prob }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ThetaScalerInputs {
    seconds_to_expiry: u64,
    period_duration_secs: u64,
    theta_decay_factor: f64,
}

fn compute_theta_scaler(inputs: &ThetaScalerInputs) -> Option<f64> {
    if !inputs.theta_decay_factor.is_finite() || inputs.theta_decay_factor < 0.0 {
        return None;
    }
    if inputs.theta_decay_factor == 0.0 {
        return Some(1.0);
    }
    if inputs.period_duration_secs == 0 {
        return None;
    }

    let ratio =
        (inputs.seconds_to_expiry as f64 / inputs.period_duration_secs as f64).clamp(0.0, 1.0);
    Some(1.0 + inputs.theta_decay_factor * (1.0 - ratio).powi(2))
}

fn compute_worst_case_ev_bps(side: OutcomeSide, inputs: &WorstCaseEvInputs) -> Option<f64> {
    let fair_probability = sanitize_probability(inputs.fair_probability?)?;
    let uncertainty_band_probability = sanitize_probability(inputs.uncertainty_band_probability)?;
    let executable_entry_cost = inputs.executable_entry_cost;
    let fee_bps = inputs.fee_bps?;

    if !executable_entry_cost.is_finite() || executable_entry_cost <= 0.0 {
        return None;
    }
    if !fee_bps.is_finite() || fee_bps < 0.0 {
        return None;
    }

    let p_lo = (fair_probability - uncertainty_band_probability).clamp(0.0, 1.0);
    let p_hi = (fair_probability + uncertainty_band_probability).clamp(0.0, 1.0);
    let worst_case_success_probability = match side {
        OutcomeSide::Up => p_lo,
        OutcomeSide::Down => 1.0 - p_hi,
    };
    let total_entry_cost = executable_entry_cost * (1.0 + fee_bps / BPS_DENOMINATOR);

    if total_entry_cost <= 0.0 {
        return None;
    }

    Some(((worst_case_success_probability - total_entry_cost) / total_entry_cost) * 10_000.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct SideSelectionInputs {
    up_worst_ev_bps: Option<f64>,
    down_worst_ev_bps: Option<f64>,
    min_worst_case_ev_bps: f64,
}

fn choose_entry_side(inputs: &SideSelectionInputs) -> Option<OutcomeSide> {
    if !inputs.min_worst_case_ev_bps.is_finite() {
        return None;
    }

    let up_worst_ev_bps = inputs.up_worst_ev_bps.filter(|value| value.is_finite())?;
    let down_worst_ev_bps = inputs.down_worst_ev_bps.filter(|value| value.is_finite())?;
    let up_clears = up_worst_ev_bps > inputs.min_worst_case_ev_bps;
    let down_clears = down_worst_ev_bps > inputs.min_worst_case_ev_bps;

    match (up_clears, down_clears) {
        (true, false) => Some(OutcomeSide::Up),
        (false, true) => Some(OutcomeSide::Down),
        (true, true) if up_worst_ev_bps > down_worst_ev_bps => Some(OutcomeSide::Up),
        (true, true) if down_worst_ev_bps > up_worst_ev_bps => Some(OutcomeSide::Down),
        (true, true) | (false, false) => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct RobustSizingInputs {
    expected_ev_per_usdc: f64,
    risk_lambda: f64,
    max_position_usdc: f64,
    impact_cap_usdc: f64,
}

fn choose_robust_size(inputs: &RobustSizingInputs) -> f64 {
    if !inputs.expected_ev_per_usdc.is_finite() || inputs.expected_ev_per_usdc <= 0.0 {
        return 0.0;
    }

    let cap = sanitize_non_negative(inputs.max_position_usdc)
        .min(sanitize_non_negative(inputs.impact_cap_usdc));
    if cap <= 0.0 {
        return 0.0;
    }

    if !inputs.risk_lambda.is_finite() || inputs.risk_lambda < 0.0 {
        return 0.0;
    }
    if inputs.risk_lambda == 0.0 {
        return cap;
    }

    (inputs.expected_ev_per_usdc / (QUADRATIC_RISK_DIVISOR * inputs.risk_lambda)).min(cap)
}

fn sanitize_probability(value: f64) -> Option<f64> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Some(value)
    } else {
        None
    }
}

fn sanitize_non_negative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExposureOccupancy {
    PendingEntry,
    EntryReconcilePending,
    ManagedPosition,
    ExitPending,
    UnsupportedObserved,
    BlindRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ForcedFlatReason {
    Freeze,
    StaleChainlink,
    ThinBook,
    MetadataMismatch,
    FastVenueIncoherent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EntryBlockReason {
    PhaseNotActive,
    MetadataMismatch,
    ActiveBookNotPriced,
    IntervalOpenMissing,
    WarmupIncomplete,
    FeesNotReady,
    RecoveryMode,
    MarketCoolingDown,
    ForcedFlat(ForcedFlatReason),
    OnePositionInvariant(ExposureOccupancy),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct EntryGateDecision {
    blocked_by: Vec<EntryBlockReason>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct EntryPricingInputs {
    spot_price: f64,
    strike_price: f64,
    seconds_to_expiry: u64,
    realized_vol: f64,
    theta_scaled_min_edge_bps: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum EntryPricingBlockReason {
    SpotPriceMissing,
    StrikePriceMissing,
    SecondsToExpiryMissing,
    RealizedVolNotReady,
    ThetaScalerUnavailable,
    UncertaintyBandUnavailable,
    FairProbabilityUnavailable,
    FeeUnavailable(OutcomeSide),
    ExecutableEntryCostUnavailable(OutcomeSide),
    WorstCaseEvUnavailable(OutcomeSide),
}

#[derive(Debug, Clone, PartialEq)]
struct EntryEvaluation {
    gate: EntryGateDecision,
    pricing_blocked_by: Vec<EntryPricingBlockReason>,
    fair_probability_up: Option<f64>,
    uncertainty_band_probability: Option<f64>,
    up_worst_case_ev_bps: Option<f64>,
    down_worst_case_ev_bps: Option<f64>,
    min_worst_case_ev_bps: Option<f64>,
    expected_ev_per_usdc: Option<f64>,
    book_impact_cap_usdc: Option<f64>,
    sized_notional_usdc: Option<f64>,
    selected_side: Option<OutcomeSide>,
}

#[derive(Debug, Clone, PartialEq)]
struct EntrySubmissionDecision {
    evaluation: EntryEvaluation,
    instrument_id: Option<InstrumentId>,
    order_side: Option<OrderSide>,
    price: Option<f64>,
    quantity_value: Option<f64>,
    client_order_id: Option<ClientOrderId>,
    blocked_reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
struct EntryEvaluationLogFields {
    market_id: Option<String>,
    phase: SelectionPhase,
    gate_blocked_by: Vec<EntryBlockReason>,
    pricing_blocked_by: Vec<EntryPricingBlockReason>,
    spot_price: Option<f64>,
    spot_venue_name: Option<String>,
    reference_fair_value: Option<f64>,
    interval_open: Option<f64>,
    seconds_to_expiry: Option<u64>,
    realized_vol: Option<f64>,
    realized_vol_source_venue: Option<String>,
    realized_vol_source_ts_ms: Option<u64>,
    pricing_kurtosis: f64,
    theta_decay_factor: f64,
    theta_scaled_min_edge_bps: Option<f64>,
    fair_probability_up: Option<f64>,
    fair_probability_down: Option<f64>,
    uncertainty_band_probability: Option<f64>,
    uncertainty_band_live: bool,
    uncertainty_band_reason: &'static str,
    lead_agreement_corr: Option<f64>,
    fast_venue_age_ms: Option<u64>,
    fast_venue_jitter_ms: Option<u64>,
    up_fee_bps: Option<f64>,
    down_fee_bps: Option<f64>,
    up_entry_cost: Option<f64>,
    down_entry_cost: Option<f64>,
    up_worst_case_ev_bps: Option<f64>,
    down_worst_case_ev_bps: Option<f64>,
    expected_ev_per_usdc: Option<f64>,
    max_position_usdc: f64,
    risk_lambda: f64,
    book_impact_cap_bps: u64,
    book_impact_cap_usdc: Option<f64>,
    sized_notional_usdc: Option<f64>,
    selected_side: Option<OutcomeSide>,
    fast_venue_available: bool,
    fast_venue_fallback_to_reference: bool,
    lead_quality_policy_applied: bool,
    lead_quality_reason: &'static str,
    maker_rebate_available: bool,
    maker_rebate_reason: &'static str,
    category_available: bool,
    category_reason: &'static str,
    final_fee_amount_known: bool,
    final_fee_amount_reason: &'static str,
    submission_instrument_id: Option<InstrumentId>,
    submission_order_side: Option<OrderSide>,
    submission_price: Option<f64>,
    submission_quantity_value: Option<f64>,
    submission_client_order_id: Option<ClientOrderId>,
    submission_blocked_reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
struct ExitEvaluation {
    position_outcome_side: Option<OutcomeSide>,
    forced_flat_reasons: Vec<ForcedFlatReason>,
    hold_ev_bps: Option<f64>,
    exit_ev_bps: Option<f64>,
    exit_decision: Option<ExitDecision>,
    blocked_reason: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
struct ExitSubmissionDecision {
    evaluation: ExitEvaluation,
    instrument_id: Option<InstrumentId>,
    order_side: Option<OrderSide>,
    price: Option<f64>,
    quantity: Option<Quantity>,
    client_order_id: Option<ClientOrderId>,
    blocked_reason: Option<&'static str>,
    forced_flat_reasons: Vec<ForcedFlatReason>,
}

#[derive(Debug, Clone, PartialEq)]
struct ExitEvaluationLogFields {
    market_id: Option<String>,
    phase: SelectionPhase,
    position_outcome_side: Option<OutcomeSide>,
    position_id: Option<PositionId>,
    position_instrument_id: Option<InstrumentId>,
    position_quantity: Option<Quantity>,
    position_avg_px_open: Option<f64>,
    forced_flat_reasons: Vec<ForcedFlatReason>,
    spot_price: Option<f64>,
    spot_venue_name: Option<String>,
    reference_fair_value: Option<f64>,
    interval_open: Option<f64>,
    seconds_to_expiry: Option<u64>,
    realized_vol: Option<f64>,
    realized_vol_source_venue: Option<String>,
    realized_vol_source_ts_ms: Option<u64>,
    pricing_kurtosis: f64,
    exit_hysteresis_bps: i64,
    fair_probability_up: Option<f64>,
    fair_probability_down: Option<f64>,
    uncertainty_band_probability: Option<f64>,
    up_fee_bps: Option<f64>,
    down_fee_bps: Option<f64>,
    hold_ev_bps: Option<f64>,
    exit_ev_bps: Option<f64>,
    exit_decision: Option<ExitDecision>,
    historical_entry_fee_rate_known: bool,
    historical_entry_fee_rate_reason: &'static str,
    maker_rebate_available: bool,
    maker_rebate_reason: &'static str,
    category_available: bool,
    category_reason: &'static str,
    final_fee_amount_known: bool,
    final_fee_amount_reason: &'static str,
    submission_instrument_id: Option<InstrumentId>,
    submission_order_side: Option<OrderSide>,
    submission_price: Option<f64>,
    submission_quantity: Option<Quantity>,
    submission_client_order_id: Option<ClientOrderId>,
    submission_blocked_reason: Option<&'static str>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
struct EntryOrderPlanInputs {
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    outcome_side: OutcomeSide,
    quantity: Quantity,
    price_precision: u8,
    best_bid: f64,
    best_ask: f64,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
struct EntryOrderPlan {
    client_order_id: ClientOrderId,
    instrument_id: InstrumentId,
    order_side: OrderSide,
    quantity: Quantity,
    price: Price,
    time_in_force: TimeInForce,
}

#[cfg(test)]
fn build_entry_order_plan(inputs: &EntryOrderPlanInputs) -> Result<EntryOrderPlan> {
    let (order_side, raw_price) = match inputs.outcome_side {
        OutcomeSide::Up => (OrderSide::Buy, inputs.best_ask),
        OutcomeSide::Down => (OrderSide::Buy, inputs.best_ask),
    };
    anyhow::ensure!(
        raw_price.is_finite() && raw_price > 0.0,
        "entry price must be positive"
    );

    Ok(EntryOrderPlan {
        client_order_id: inputs.client_order_id,
        instrument_id: inputs.instrument_id,
        order_side,
        quantity: inputs.quantity,
        price: Price::new(raw_price, inputs.price_precision),
        time_in_force: TimeInForce::Fok,
    })
}

fn order_submission_facts(
    order_type: OrderType,
    time_in_force: TimeInForce,
    instrument_id: InstrumentId,
    order_side: OrderSide,
    price: Price,
    quantity: Quantity,
    is_reduce_only: bool,
    client_order_id: ClientOrderId,
) -> Result<BoltV3OrderSubmissionFacts> {
    Ok(BoltV3OrderSubmissionFacts {
        order_type: order_type_as_decision_fact(order_type)?.to_string(),
        time_in_force: time_in_force_as_decision_fact(time_in_force)?.to_string(),
        instrument_id: instrument_id.to_string(),
        side: order_side_as_decision_fact(order_side)?.to_string(),
        price: price.as_f64(),
        quantity: quantity.as_f64(),
        is_quote_quantity: false,
        is_post_only: false,
        is_reduce_only,
        client_order_id: Some(client_order_id.to_string()),
    })
}

fn entry_pre_submit_rejection_facts(
    decision: &EntrySubmissionDecision,
) -> Result<Option<BoltV3PreSubmitRejectionFacts>> {
    let Some(rejection_reason) = decision.blocked_reason else {
        return Ok(None);
    };
    if rejection_reason != "instrument_missing_from_cache" {
        return Ok(None);
    }

    let side = decision
        .order_side
        .map(order_side_as_decision_fact)
        .transpose()?
        .map(str::to_string);

    Ok(Some(BoltV3PreSubmitRejectionFacts {
        order: BoltV3RejectedOrderFacts {
            order_type: None,
            time_in_force: None,
            instrument_id: decision
                .instrument_id
                .as_ref()
                .map(std::string::ToString::to_string),
            side,
            price: decision.price,
            quantity: decision.quantity_value,
            is_quote_quantity: None,
            is_post_only: None,
            is_reduce_only: None,
            client_order_id: decision
                .client_order_id
                .as_ref()
                .map(std::string::ToString::to_string),
        },
        rejection_reason: rejection_reason.to_string(),
        authoritative_position_quantity: None,
        authoritative_sellable_quantity: None,
        open_exit_order_quantity: None,
        uncovered_position_quantity: None,
    }))
}

fn exit_pre_submit_rejection_facts(
    decision: &ExitSubmissionDecision,
    position_quantity: Option<f64>,
) -> Result<Option<BoltV3PreSubmitRejectionFacts>> {
    let Some(rejection_reason) = decision.blocked_reason else {
        return Ok(None);
    };
    if rejection_reason != "exit_price_missing" {
        return Ok(None);
    }

    let side = decision
        .order_side
        .map(order_side_as_decision_fact)
        .transpose()?
        .map(str::to_string);

    Ok(Some(BoltV3PreSubmitRejectionFacts {
        order: BoltV3RejectedOrderFacts {
            order_type: None,
            time_in_force: None,
            instrument_id: decision
                .instrument_id
                .as_ref()
                .map(std::string::ToString::to_string),
            side,
            price: decision.price,
            quantity: decision.quantity.map(|quantity| quantity.as_f64()),
            is_quote_quantity: None,
            is_post_only: None,
            is_reduce_only: None,
            client_order_id: decision
                .client_order_id
                .as_ref()
                .map(std::string::ToString::to_string),
        },
        rejection_reason: rejection_reason.to_string(),
        authoritative_position_quantity: position_quantity,
        authoritative_sellable_quantity: decision.quantity.map(|quantity| quantity.as_f64()),
        open_exit_order_quantity: Some(0.0),
        uncovered_position_quantity: position_quantity.map(|quantity| quantity.max(0.0)),
    }))
}

fn order_type_as_decision_fact(order_type: OrderType) -> Result<&'static str> {
    match order_type {
        OrderType::Limit => Ok("limit"),
        OrderType::Market => Ok("market"),
        _ => anyhow::bail!("unsupported order type for bolt-v3 decision event: {order_type:?}"),
    }
}

fn time_in_force_as_decision_fact(time_in_force: TimeInForce) -> Result<&'static str> {
    match time_in_force {
        TimeInForce::Gtc => Ok("gtc"),
        TimeInForce::Fok => Ok("fok"),
        TimeInForce::Ioc => Ok("ioc"),
        _ => {
            anyhow::bail!("unsupported time-in-force for bolt-v3 decision event: {time_in_force:?}")
        }
    }
}

fn order_side_as_decision_fact(order_side: OrderSide) -> Result<&'static str> {
    match order_side {
        OrderSide::Buy => Ok("buy"),
        OrderSide::Sell => Ok("sell"),
        _ => anyhow::bail!("unsupported order side for bolt-v3 decision event: {order_side:?}"),
    }
}

fn outcome_side_as_decision_fact(outcome_side: OutcomeSide) -> &'static str {
    match outcome_side {
        OutcomeSide::Up => "up",
        OutcomeSide::Down => "down",
    }
}

fn forced_flat_reason_as_decision_fact(reason: &ForcedFlatReason) -> &'static str {
    match reason {
        ForcedFlatReason::Freeze => "freeze",
        ForcedFlatReason::StaleChainlink => "stale_reference_quote",
        ForcedFlatReason::ThinBook => "thin_book",
        ForcedFlatReason::MetadataMismatch => "metadata_mismatch",
        ForcedFlatReason::FastVenueIncoherent => "fast_venue_incoherent",
    }
}

fn forced_flat_reasons_as_decision_facts(reasons: &[ForcedFlatReason]) -> Vec<&'static str> {
    reasons
        .iter()
        .map(forced_flat_reason_as_decision_fact)
        .collect()
}

fn exit_decision_reason_as_decision_fact(
    decision: ExitDecision,
    forced_flat_reasons: &[ForcedFlatReason],
) -> Option<&'static str> {
    match decision {
        ExitDecision::Exit if !forced_flat_reasons.is_empty() => Some("forced_flat"),
        ExitDecision::Exit => Some("ev_hysteresis"),
        ExitDecision::ExitFailClosed => Some("fail_closed"),
        ExitDecision::Hold => None,
    }
}

fn unix_nanos_from_millis(ts_ms: u64) -> Result<UnixNanos> {
    let ts_ns = ts_ms
        .checked_mul(1_000_000)
        .ok_or_else(|| anyhow::anyhow!("timestamp milliseconds overflows nanoseconds: {ts_ms}"))?;
    Ok(UnixNanos::from(ts_ns))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitDecision {
    Hold,
    Exit,
    ExitFailClosed,
}

fn should_report_one_position_gate_violation(occupancy: ExposureOccupancy) -> bool {
    matches!(
        occupancy,
        ExposureOccupancy::EntryReconcilePending
            | ExposureOccupancy::UnsupportedObserved
            | ExposureOccupancy::BlindRecovery
    )
}

fn should_warn_on_exit_submission_block(reason: Option<&str>) -> bool {
    !matches!(
        reason,
        Some("no_open_position" | "exit_already_pending" | "exit_hold")
    )
}

fn evaluate_exit_decision(
    hold_ev_bps: Option<f64>,
    exit_ev_bps: Option<f64>,
    exit_hysteresis_bps: f64,
) -> ExitDecision {
    let Some(hold_ev_bps) = hold_ev_bps.filter(|value| value.is_finite()) else {
        return ExitDecision::ExitFailClosed;
    };
    let Some(exit_ev_bps) = exit_ev_bps.filter(|value| value.is_finite()) else {
        return ExitDecision::ExitFailClosed;
    };
    if !exit_hysteresis_bps.is_finite() {
        return ExitDecision::ExitFailClosed;
    }

    if exit_ev_bps >= hold_ev_bps - exit_hysteresis_bps {
        ExitDecision::Exit
    } else {
        ExitDecision::Hold
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ForcedFlatInputs {
    phase: SelectionPhase,
    metadata_matches_selection: bool,
    last_chainlink_ts_ms: Option<u64>,
    now_ms: u64,
    stale_chainlink_after_ms: u64,
    liquidity_available: Option<f64>,
    min_liquidity_required: f64,
    fast_venue_incoherent: bool,
}

fn evaluate_forced_flat_predicates(inputs: &ForcedFlatInputs) -> Vec<ForcedFlatReason> {
    let mut reasons = Vec::new();
    let chainlink_stale = inputs.last_chainlink_ts_ms.is_some_and(|last_ts_ms| {
        inputs.now_ms.saturating_sub(last_ts_ms) > inputs.stale_chainlink_after_ms
    });

    if inputs.phase == SelectionPhase::Freeze {
        reasons.push(ForcedFlatReason::Freeze);
    }
    if chainlink_stale {
        reasons.push(ForcedFlatReason::StaleChainlink);
    }
    if inputs.liquidity_available.is_some_and(|liquidity| {
        !liquidity.is_finite() || liquidity < inputs.min_liquidity_required
    }) {
        reasons.push(ForcedFlatReason::ThinBook);
    }
    if !inputs.metadata_matches_selection {
        reasons.push(ForcedFlatReason::MetadataMismatch);
    }
    if inputs.fast_venue_incoherent && chainlink_stale {
        reasons.push(ForcedFlatReason::FastVenueIncoherent);
    }

    reasons
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
    use nautilus_model::types::Quantity;
    use rust_decimal::Decimal;

    use super::*;
    use crate::{
        platform::{
            reference::{EffectiveVenueState, ReferenceSnapshot, VenueHealth, VenueKind},
            resolution_basis::parse_ruleset_resolution_basis,
            ruleset::{
                CandidateMarket, RuntimeSelectionSnapshot, SelectionDecision, SelectionState,
            },
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
            period_duration_secs = 300
            reentry_cooldown_secs = 30
            max_position_usdc = 1000.0
            book_impact_cap_bps = 15
            risk_lambda = 0.5
            worst_case_ev_min_bps = -20
            exit_hysteresis_bps = 5
            vol_window_secs = 60
            vol_gap_reset_secs = 10
            vol_min_observations = 20
            vol_bridge_valid_secs = 10
            pricing_kurtosis = 0.0
            theta_decay_factor = 0.0
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
                period_duration_secs: 300,
                reentry_cooldown_secs: 30,
                max_position_usdc: 1000.0,
                book_impact_cap_bps: 15,
                risk_lambda: 0.5,
                worst_case_ev_min_bps: -20,
                exit_hysteresis_bps: 5,
                vol_window_secs: 60,
                vol_gap_reset_secs: 10,
                vol_min_observations: 20,
                vol_bridge_valid_secs: 10,
                pricing_kurtosis: 0.0,
                theta_decay_factor: 0.0,
                forced_flat_stale_chainlink_ms: 1500,
                forced_flat_thin_book_min_liquidity: 100.0,
                lead_agreement_min_corr: 0.8,
                lead_jitter_max_ms: 250,
            },
            StrategyBuildContext {
                fee_provider,
                reference_publish_topic: "platform.reference.test.chainlink".to_string(),
                bolt_v3_decision_evidence: None,
                bolt_v3_market_selection_context: None,
            },
        )
    }

    fn ready_to_trade_strategy() -> EthChainlinkTaker {
        let mut strategy = test_strategy();
        strategy.config.warmup_tick_count = 2;
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
        strategy.active.interval_open = Some(3_100.0);
        strategy.active.warmup_count = 2;
        strategy.active.last_reference_ts_ms = Some(1_200);
        strategy.active.outcome_fees.up_ready = true;
        strategy.active.outcome_fees.down_ready = true;
        strategy.active.books.up.last_observed_instrument_id =
            strategy.active.books.up.instrument_id;
        strategy
            .active
            .books
            .up
            .bid_levels
            .insert(Price::new(0.43, 2), 500.0);
        strategy
            .active
            .books
            .up
            .ask_levels
            .insert(Price::new(0.45, 2), 500.0);
        strategy.active.books.up.best_bid = Some(0.43);
        strategy.active.books.up.best_ask = Some(0.45);
        strategy.active.books.up.liquidity_available = Some(500.0);
        strategy.active.books.down.last_observed_instrument_id =
            strategy.active.books.down.instrument_id;
        strategy
            .active
            .books
            .down
            .bid_levels
            .insert(Price::new(0.43, 2), 500.0);
        strategy
            .active
            .books
            .down
            .ask_levels
            .insert(Price::new(0.45, 2), 500.0);
        strategy.active.books.down.best_bid = Some(0.43);
        strategy.active.books.down.best_ask = Some(0.45);
        strategy.active.books.down.liquidity_available = Some(500.0);
        strategy.active.fast_venue_incoherent = false;
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_100.5, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = Some(1.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
        strategy
    }

    fn ready_to_trade_strategy_with_live_fees(
        up_fee_bps: Decimal,
        down_fee_bps: Decimal,
    ) -> EthChainlinkTaker {
        ready_to_trade_strategy_with_recording_fees(up_fee_bps, down_fee_bps).0
    }

    fn ready_to_trade_strategy_with_recording_fees(
        up_fee_bps: Decimal,
        down_fee_bps: Decimal,
    ) -> (EthChainlinkTaker, Arc<RecordingFeeProvider>) {
        let fee_provider = RecordingFeeProvider::cold();
        fee_provider.set_fee("MKT-1-UP", up_fee_bps);
        fee_provider.set_fee("MKT-1-DOWN", down_fee_bps);

        let mut strategy = test_strategy_with_fee_provider(fee_provider.clone());
        strategy.config.warmup_tick_count = 2;
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
        strategy.active.interval_open = Some(3_100.0);
        strategy.active.warmup_count = 2;
        strategy.active.last_reference_ts_ms = Some(1_200);
        strategy.refresh_fee_readiness();
        strategy.active.books.up.last_observed_instrument_id =
            strategy.active.books.up.instrument_id;
        strategy
            .active
            .books
            .up
            .bid_levels
            .insert(Price::new(0.50, 2), 500.0);
        strategy
            .active
            .books
            .up
            .ask_levels
            .insert(Price::new(0.50, 2), 500.0);
        strategy.active.books.up.best_bid = Some(0.50);
        strategy.active.books.up.best_ask = Some(0.50);
        strategy.active.books.up.liquidity_available = Some(500.0);
        strategy.active.books.down.last_observed_instrument_id =
            strategy.active.books.down.instrument_id;
        strategy
            .active
            .books
            .down
            .bid_levels
            .insert(Price::new(0.48, 2), 500.0);
        strategy
            .active
            .books
            .down
            .ask_levels
            .insert(Price::new(0.49, 2), 500.0);
        strategy.active.books.down.best_bid = Some(0.48);
        strategy.active.books.down.best_ask = Some(0.49);
        strategy.active.books.down.liquidity_available = Some(500.0);
        strategy.active.fast_venue_incoherent = false;
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_100.5, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = Some(1.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
        (strategy, fee_provider)
    }

    fn pending_entry_state(
        strategy: &EthChainlinkTaker,
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
        outcome_side: OutcomeSide,
        book: OutcomeBookState,
    ) -> PendingEntryState {
        PendingEntryState {
            client_order_id,
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            outcome_side: Some(outcome_side),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: strategy.outcome_fee_bps(outcome_side).or(Some(0.0)),
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book,
        }
    }

    fn set_pending_entry(strategy: &mut EthChainlinkTaker, pending: PendingEntryState) {
        strategy.exposure = ExposureState::PendingEntry(pending);
    }

    fn set_entry_reconcile_pending(
        strategy: &mut EthChainlinkTaker,
        pending: PendingEntryState,
        reason: EntryReconcileReason,
    ) {
        strategy.exposure = ExposureState::EntryReconcilePending { pending, reason };
    }

    fn set_managed_position(
        strategy: &mut EthChainlinkTaker,
        position: OpenPositionState,
        origin: ManagedPositionOrigin,
    ) {
        strategy.exposure = ExposureState::Managed(ManagedPositionState { position, origin });
    }

    fn set_exit_pending(
        strategy: &mut EthChainlinkTaker,
        position: OpenPositionState,
        client_order_id: ClientOrderId,
        fill_received: bool,
        close_received: bool,
        origin: ManagedPositionOrigin,
    ) {
        strategy.exposure = ExposureState::ExitPending(ExitPendingState {
            pending_exit: PendingExitState {
                client_order_id,
                market_id: position.market_id.clone(),
                position_id: Some(position.position_id),
                fill_received,
                close_received,
            },
            position: Some(ManagedPositionState { position, origin }),
        });
    }

    fn set_blind_recovery(strategy: &mut EthChainlinkTaker, reason: BlindRecoveryReason) {
        strategy.exposure = ExposureState::BlindRecovery(BlindRecoveryState { reason });
    }

    fn set_unsupported_observed(
        strategy: &mut EthChainlinkTaker,
        observed: OpenPositionState,
        reason: UnsupportedObservedReason,
    ) {
        strategy.exposure =
            ExposureState::UnsupportedObserved(UnsupportedObservedState { observed, reason });
    }

    fn managed_position_ref(strategy: &EthChainlinkTaker) -> Option<&OpenPositionState> {
        strategy.managed_position().map(|managed| &managed.position)
    }

    fn pending_exit_ref(strategy: &EthChainlinkTaker) -> Option<&PendingExitState> {
        strategy
            .exposure
            .exit_pending()
            .map(|exit_pending| &exit_pending.pending_exit)
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

    fn selection_snapshot(
        interval_start_ms: u64,
        state: SelectionState,
    ) -> RuntimeSelectionSnapshot {
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
            market_slug: market_id.to_string(),
            question_id: format!("question-{market_id}"),
            instrument_id: polymarket_instrument_id(&condition_id, &up_token_id).to_string(),
            condition_id,
            up_token_id,
            down_token_id,
            selected_market_observed_ts_ms: interval_start_ms,
            price_to_beat: None,
            price_to_beat_source: None,
            price_to_beat_observed_ts_ms: None,
            start_ts_ms: interval_start_ms,
            end_ts_ms: interval_start_ms + 300_000,
            declared_resolution_basis: parse_ruleset_resolution_basis("chainlink_ethusd")
                .expect("test resolution basis should parse"),
            accepting_orders: true,
            liquidity_num: 1000.0,
            seconds_to_end: 300,
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

    fn orderbook_venue(
        venue_name: &str,
        effective_weight: f64,
        price: f64,
        observed_ts_ms: u64,
    ) -> EffectiveVenueState {
        EffectiveVenueState {
            venue_name: venue_name.to_string(),
            base_weight: effective_weight,
            effective_weight,
            stale: false,
            health: VenueHealth::Healthy,
            observed_ts_ms: Some(observed_ts_ms),
            venue_kind: VenueKind::Orderbook,
            observed_price: Some(price),
            observed_bid: Some(price - 0.01),
            observed_ask: Some(price + 0.01),
        }
    }

    fn oracle_venue(
        venue_name: &str,
        effective_weight: f64,
        price: f64,
        observed_ts_ms: u64,
    ) -> EffectiveVenueState {
        EffectiveVenueState {
            venue_name: venue_name.to_string(),
            base_weight: effective_weight,
            effective_weight,
            stale: false,
            health: VenueHealth::Healthy,
            observed_ts_ms: Some(observed_ts_ms),
            venue_kind: VenueKind::Oracle,
            observed_price: Some(price),
            observed_bid: None,
            observed_ask: None,
        }
    }

    fn fast_spot(venue_name: &str, price: f64, observed_ts_ms: u64) -> FastSpotObservation {
        FastSpotObservation {
            venue_name: venue_name.to_string(),
            price,
            observed_ts_ms,
        }
    }

    fn book_deltas(
        instrument_id: InstrumentId,
        deltas: &[(BookAction, OrderSide, f64, f64)],
    ) -> nautilus_model::data::OrderBookDeltas {
        let deltas = deltas
            .iter()
            .map(|(action, side, price, size)| {
                nautilus_model::data::OrderBookDelta::new_checked(
                    instrument_id,
                    *action,
                    nautilus_model::data::BookOrder::new(
                        *side,
                        Price::new(*price, 2),
                        Quantity::new(*size, 2),
                        0,
                    ),
                    0,
                    0,
                    0.into(),
                    0.into(),
                )
                .expect("test order book delta should build")
            })
            .collect();

        nautilus_model::data::OrderBookDeltas::new(instrument_id, deltas)
    }

    fn lead_signal(
        venue_name: &str,
        age_ms: u64,
        jitter_ms: u64,
        agreement_corr: f64,
        effective_weight: f64,
        lead_gap_probability: f64,
    ) -> LeadVenueSignal {
        LeadVenueSignal {
            venue_name: venue_name.to_string(),
            price: Some(3_100.0),
            observed_ts_ms: Some(1_000),
            age_ms,
            jitter_ms,
            agreement_corr,
            effective_weight,
            lead_gap_probability,
        }
    }

    fn position_opened_event(
        instrument_id: InstrumentId,
        position_id: PositionId,
        quantity: Quantity,
        avg_px_open: f64,
    ) -> nautilus_model::events::PositionOpened {
        position_opened_event_with_details(
            instrument_id,
            position_id,
            quantity,
            avg_px_open,
            OrderSide::Buy,
            PositionSide::Long,
        )
    }

    fn position_opened_event_with_details(
        instrument_id: InstrumentId,
        position_id: PositionId,
        quantity: Quantity,
        avg_px_open: f64,
        entry: OrderSide,
        side: PositionSide,
    ) -> nautilus_model::events::PositionOpened {
        nautilus_model::events::PositionOpened {
            trader_id: nautilus_model::identifiers::TraderId::from("TRADER-001"),
            strategy_id: StrategyId::from("ETHCHAINLINKTAKER-001"),
            instrument_id,
            position_id,
            account_id: nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT"),
            opening_order_id: ClientOrderId::from("ENTRY-001"),
            entry,
            side,
            signed_qty: quantity.as_f64(),
            quantity,
            last_qty: quantity,
            last_px: Price::new(avg_px_open, 3),
            currency: nautilus_model::types::Currency::USDC(),
            avg_px_open,
            event_id: nautilus_core::UUID4::new(),
            ts_event: nautilus_core::UnixNanos::from(1_u64),
            ts_init: nautilus_core::UnixNanos::from(1_u64),
        }
    }

    fn order_filled_event(
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
        position_id: PositionId,
    ) -> nautilus_model::events::OrderFilled {
        order_filled_event_with_details(
            client_order_id,
            instrument_id,
            Some(position_id),
            OrderSide::Buy,
        )
    }

    fn order_filled_event_with_details(
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
        position_id: Option<PositionId>,
        order_side: OrderSide,
    ) -> nautilus_model::events::OrderFilled {
        let mut fill = nautilus_model::events::OrderFilled::new(
            nautilus_model::identifiers::TraderId::from("TRADER-001"),
            StrategyId::from("ETHCHAINLINKTAKER-001"),
            instrument_id,
            client_order_id,
            nautilus_model::identifiers::VenueOrderId::from("V-ORDER-001"),
            nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT"),
            nautilus_model::identifiers::TradeId::from("TRADE-001"),
            order_side,
            nautilus_model::enums::OrderType::Limit,
            Quantity::new(10.0, 2),
            Price::new(0.450, 3),
            nautilus_model::types::Currency::USDC(),
            nautilus_model::enums::LiquiditySide::Taker,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_000_u64),
            nautilus_core::UnixNanos::from(1_000_u64),
            false,
            None,
            Some(nautilus_model::types::Money::new(
                0.0,
                nautilus_model::types::Currency::USDC(),
            )),
        );
        fill.position_id = position_id;
        fill
    }

    fn order_canceled_event(
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
    ) -> nautilus_model::events::OrderCanceled {
        nautilus_model::events::OrderCanceled::new(
            nautilus_model::identifiers::TraderId::from("TRADER-001"),
            StrategyId::from("ETHCHAINLINKTAKER-001"),
            instrument_id,
            client_order_id,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_000_u64),
            nautilus_core::UnixNanos::from(1_000_u64),
            false,
            Some(nautilus_model::identifiers::VenueOrderId::from(
                "V-ORDER-001",
            )),
            Some(nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT")),
        )
    }

    fn order_rejected_event(
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
    ) -> nautilus_model::events::OrderRejected {
        nautilus_model::events::OrderRejected::new(
            nautilus_model::identifiers::TraderId::from("TRADER-001"),
            StrategyId::from("ETHCHAINLINKTAKER-001"),
            instrument_id,
            client_order_id,
            nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT"),
            "rejected".into(),
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_000_u64),
            nautilus_core::UnixNanos::from(1_000_u64),
            false,
            false,
        )
    }

    fn order_expired_event(
        client_order_id: ClientOrderId,
        instrument_id: InstrumentId,
    ) -> nautilus_model::events::OrderExpired {
        nautilus_model::events::OrderExpired::new(
            nautilus_model::identifiers::TraderId::from("TRADER-001"),
            StrategyId::from("ETHCHAINLINKTAKER-001"),
            instrument_id,
            client_order_id,
            nautilus_core::UUID4::new(),
            nautilus_core::UnixNanos::from(1_000_u64),
            nautilus_core::UnixNanos::from(1_000_u64),
            false,
            Some(nautilus_model::identifiers::VenueOrderId::from(
                "V-ORDER-001",
            )),
            Some(nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT")),
        )
    }

    fn position_closed_event(
        instrument_id: InstrumentId,
        position_id: PositionId,
    ) -> nautilus_model::events::PositionClosed {
        nautilus_model::events::PositionClosed {
            trader_id: nautilus_model::identifiers::TraderId::from("TRADER-001"),
            strategy_id: StrategyId::from("ETHCHAINLINKTAKER-001"),
            instrument_id,
            position_id,
            account_id: nautilus_model::identifiers::AccountId::from("TEST-ACCOUNT"),
            opening_order_id: ClientOrderId::from("ENTRY-001"),
            closing_order_id: Some(ClientOrderId::from("EXIT-001")),
            entry: OrderSide::Buy,
            side: PositionSide::Long,
            signed_qty: 0.0,
            quantity: Quantity::zero(2),
            peak_quantity: Quantity::new(10.0, 2),
            last_qty: Quantity::new(10.0, 2),
            last_px: Price::new(0.550, 3),
            currency: nautilus_model::types::Currency::USDC(),
            avg_px_open: 0.450,
            avg_px_close: Some(0.550),
            realized_return: 0.0,
            realized_pnl: None,
            unrealized_pnl: nautilus_model::types::Money::new(
                0.0,
                nautilus_model::types::Currency::USDC(),
            ),
            duration: nautilus_core::nanos::DurationNanos::from(1_u64),
            event_id: nautilus_core::UUID4::new(),
            ts_opened: nautilus_core::UnixNanos::from(1_u64),
            ts_closed: Some(nautilus_core::UnixNanos::from(2_u64)),
            ts_event: nautilus_core::UnixNanos::from(2_u64),
            ts_init: nautilus_core::UnixNanos::from(2_u64),
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
            period_duration_secs = 300
            reentry_cooldown_secs = 30
            max_position_usdc = 1000
            book_impact_cap_bps = 15
            risk_lambda = 1
            worst_case_ev_min_bps = -20
            exit_hysteresis_bps = 5
            vol_window_secs = 60
            vol_gap_reset_secs = 10
            vol_min_observations = 20
            vol_bridge_valid_secs = 10
            pricing_kurtosis = 0
            theta_decay_factor = 0
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
    fn builder_requires_pricing_model_fields() {
        let raw = toml::toml! {
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
        .into();
        let mut errors = Vec::new();

        EthChainlinkTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.period_duration_secs")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.vol_window_secs")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.vol_gap_reset_secs")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.vol_min_observations")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.vol_bridge_valid_secs")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.pricing_kurtosis")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.theta_decay_factor")
        );
    }

    #[test]
    fn pricing_state_prefers_fast_spot_and_falls_back_to_reference() {
        let config = test_strategy().config.clone();
        let mut pricing = PricingState::from_config(&config);

        pricing.observe_reference_snapshot(
            &reference_tick(1_000, 3_100.0),
            config.lead_agreement_min_corr,
            config.lead_jitter_max_ms,
        );
        assert_eq!(pricing.spot_price(), Some(3_100.0));

        let snapshot = ReferenceSnapshot {
            ts_ms: 1_100,
            topic: "platform.reference.test.chainlink".to_string(),
            fair_value: Some(3_101.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("chainlink", 1.0, 3_101.0, 1_100),
                orderbook_venue("bybit", 0.9, 3_102.0, 1_100),
            ],
        };
        pricing.observe_reference_snapshot(
            &snapshot,
            config.lead_agreement_min_corr,
            config.lead_jitter_max_ms,
        );
        assert_eq!(pricing.spot_price(), Some(3_102.0));
    }

    #[test]
    fn pricing_state_applies_lead_quality_thresholds() {
        let mut config = test_strategy().config.clone();
        config.lead_agreement_min_corr = 0.9999;
        let mut pricing = PricingState::from_config(&config);

        let snapshot = ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.chainlink".to_string(),
            fair_value: Some(3_100.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("chainlink", 1.0, 3_100.0, 1_000),
                orderbook_venue("bybit", 0.9, 3_102.0, 1_000),
            ],
        };

        pricing.observe_reference_snapshot(
            &snapshot,
            config.lead_agreement_min_corr,
            config.lead_jitter_max_ms,
        );

        assert!(pricing.fast_spot.is_none());
        assert!(pricing.fast_venue_incoherent);
        assert_eq!(pricing.spot_price(), Some(3_100.0));
    }

    #[test]
    fn pricing_state_clears_fast_spot_when_no_fast_venue_remains() {
        let config = test_strategy().config.clone();
        let mut pricing = PricingState::from_config(&config);

        pricing.observe_reference_snapshot(
            &ReferenceSnapshot {
                ts_ms: 1_000,
                topic: "platform.reference.test.chainlink".to_string(),
                fair_value: Some(3_100.0),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("chainlink", 1.0, 3_100.0, 1_000),
                    orderbook_venue("bybit", 0.9, 3_102.0, 1_000),
                ],
            },
            config.lead_agreement_min_corr,
            config.lead_jitter_max_ms,
        );
        assert_eq!(pricing.spot_price(), Some(3_102.0));

        pricing.observe_reference_snapshot(
            &ReferenceSnapshot {
                ts_ms: 1_100,
                topic: "platform.reference.test.chainlink".to_string(),
                fair_value: Some(3_101.0),
                confidence: 1.0,
                venues: vec![oracle_venue("chainlink", 1.0, 3_101.0, 1_100)],
            },
            config.lead_agreement_min_corr,
            config.lead_jitter_max_ms,
        );

        assert!(pricing.fast_spot.is_none());
        assert_eq!(pricing.spot_price(), Some(3_101.0));
    }

    #[test]
    fn outcome_book_state_applies_incremental_deltas_without_retaining_stale_levels() {
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
        let mut state = OutcomeBookState::from_instrument_id(instrument_id);

        state.update_from_deltas(&book_deltas(
            instrument_id,
            &[
                (BookAction::Update, OrderSide::Buy, 0.43, 10.0),
                (BookAction::Update, OrderSide::Sell, 0.45, 12.0),
            ],
        ));
        assert_eq!(state.best_bid, Some(0.43));
        assert_eq!(state.best_ask, Some(0.45));

        state.update_from_deltas(&book_deltas(
            instrument_id,
            &[(BookAction::Delete, OrderSide::Buy, 0.43, 0.0)],
        ));

        assert_eq!(state.best_bid, None);
        assert_eq!(state.best_ask, Some(0.45));
        assert_eq!(state.liquidity_available, Some(12.0));
    }

    #[test]
    fn realized_vol_estimator_warms_bridges_and_resets_after_gap() {
        let mut config = test_strategy().config.clone();
        config.vol_window_secs = 60;
        config.vol_gap_reset_secs = 10;
        config.vol_min_observations = 3;
        config.vol_bridge_valid_secs = 10;
        let mut estimator = RealizedVolEstimator::from_config(&config);

        assert!(estimator.observe(&fast_spot("bybit", 3_100.0, 0)).is_none());
        assert!(
            estimator
                .observe(&fast_spot("bybit", 3_101.0, 1_000))
                .is_none()
        );
        assert!(
            estimator
                .observe(&fast_spot("bybit", 3_099.5, 2_000))
                .is_none()
        );
        let ready_vol = estimator
            .observe(&fast_spot("bybit", 3_102.0, 3_000))
            .expect("vol should be ready after min observations");
        assert!(ready_vol > 0.0);
        assert_eq!(estimator.current_vol_at(12_000), Some(ready_vol));
        assert!(estimator.current_vol_at(13_001).is_none());

        assert!(
            estimator
                .observe(&fast_spot("bybit", 3_103.0, 20_000))
                .is_none()
        );
        assert_eq!(estimator.samples.len(), 1);
        assert!(estimator.last_ready_vol.is_none());
    }

    #[test]
    fn realized_vol_estimator_ignores_non_monotonic_samples_within_same_venue() {
        let mut config = test_strategy().config.clone();
        config.vol_min_observations = 1;
        let mut estimator = RealizedVolEstimator::from_config(&config);

        assert!(
            estimator
                .observe(&fast_spot("bybit", 3_100.0, 1_000))
                .is_none()
        );
        let ready_vol = estimator
            .observe(&fast_spot("bybit", 3_101.0, 2_000))
            .expect("vol should be ready after min observations");
        let sample_count = estimator.samples.len();

        assert_eq!(
            estimator.observe(&fast_spot("bybit", 3_200.0, 1_500)),
            Some(ready_vol)
        );
        assert_eq!(estimator.samples.len(), sample_count);
        assert_eq!(
            estimator.samples.back().map(|sample| sample.ts_ms),
            Some(2_000)
        );
        assert_eq!(estimator.last_ready_ts_ms, Some(2_000));
    }

    #[test]
    fn selected_realized_vol_for_candidate_falls_closed_when_state_is_missing() {
        let config = test_strategy().config.clone();
        let pricing = PricingState::from_config(&config);

        let estimator = pricing
            .selected_realized_vol_for_candidate(&lead_signal("bybit", 0, 0, 1.0, 1.0, 0.01));

        assert!(estimator.last_ready_vol.is_none());
        assert_eq!(estimator.current_vol_at(1_000), None);
    }

    #[test]
    fn realized_vol_warms_across_lead_venue_switches_when_each_venue_has_history() {
        let mut strategy = ready_to_trade_strategy();
        strategy.config.vol_min_observations = 3;
        strategy.pricing = PricingState::from_config(&strategy.config);

        for (ts_ms, venue_name, fair_value, fast_price) in [
            (1_000, "bybit", 3_100.0, 3_100.0),
            (1_100, "okx", 3_100.2, 3_100.2),
            (2_000, "bybit", 3_101.0, 3_101.0),
            (2_100, "okx", 3_101.2, 3_101.2),
            (3_000, "bybit", 3_102.0, 3_102.0),
            (3_100, "okx", 3_102.2, 3_102.2),
            (4_000, "bybit", 3_103.0, 3_103.0),
        ] {
            strategy.observe_reference_snapshot(&ReferenceSnapshot {
                ts_ms,
                topic: "platform.reference.test.chainlink".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("chainlink", 1.0, fair_value, ts_ms),
                    orderbook_venue(venue_name, 0.9, fast_price, ts_ms),
                ],
            });
        }

        assert_eq!(
            strategy.pricing.fast_spot,
            Some(fast_spot("bybit", 3_103.0, 4_000))
        );
        assert!(
            strategy.current_realized_vol_at(4_000).is_some(),
            "selected venue should be able to reuse its own warmed history across lead switches"
        );
    }

    #[test]
    fn realized_vol_warms_for_eligible_nonlead_candidates_before_selection() {
        let mut strategy = ready_to_trade_strategy();
        strategy.config.vol_min_observations = 2;
        strategy.config.lead_agreement_min_corr = 0.999;
        strategy.pricing = PricingState::from_config(&strategy.config);

        for (ts_ms, fair_value, bybit_price, okx_price) in [
            (1_000, 3_100.0, 3_100.0, 3_100.3),
            (2_000, 3_101.0, 3_101.0, 3_101.3),
            (3_000, 3_102.0, 3_102.0, 3_102.3),
            (4_000, 3_103.0, 3_103.0, 3_103.3),
        ] {
            strategy.observe_reference_snapshot(&ReferenceSnapshot {
                ts_ms,
                topic: "platform.reference.test.chainlink".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("chainlink", 1.0, fair_value, ts_ms),
                    orderbook_venue("bybit", 0.9, bybit_price, ts_ms),
                    orderbook_venue("okx", 0.8, okx_price, ts_ms),
                ],
            });
        }

        assert_eq!(
            strategy.pricing.fast_spot,
            Some(fast_spot("bybit", 3_103.0, 4_000))
        );
        assert!(
            strategy
                .pricing
                .realized_vol_by_venue
                .get("okx")
                .is_some_and(|estimator| estimator.current_vol_at(4_000).is_some()),
            "eligible non-lead venues should keep warming their own realized-vol state"
        );

        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms: 5_000,
            topic: "platform.reference.test.chainlink".to_string(),
            fair_value: Some(3_104.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("chainlink", 1.0, 3_104.0, 5_000),
                orderbook_venue("okx", 0.8, 3_104.3, 5_000),
            ],
        });

        assert_eq!(
            strategy.pricing.fast_spot,
            Some(fast_spot("okx", 3_104.3, 5_000))
        );
        assert!(
            strategy.current_realized_vol_at(5_000).is_some(),
            "an eligible venue should be ready immediately once it becomes the selected lead"
        );
    }

    #[test]
    fn realized_vol_does_not_prewarm_ineligible_nonlead_candidates() {
        let mut strategy = ready_to_trade_strategy();
        strategy.config.vol_min_observations = 2;
        strategy.config.lead_agreement_min_corr = 0.999;
        strategy.pricing = PricingState::from_config(&strategy.config);

        for (ts_ms, fair_value, bybit_price, okx_price) in [
            (1_000, 3_100.0, 3_100.0, 3_000.0),
            (2_000, 3_101.0, 3_101.0, 3_001.0),
            (3_000, 3_102.0, 3_102.0, 3_002.0),
        ] {
            strategy.observe_reference_snapshot(&ReferenceSnapshot {
                ts_ms,
                topic: "platform.reference.test.chainlink".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("chainlink", 1.0, fair_value, ts_ms),
                    orderbook_venue("bybit", 0.9, bybit_price, ts_ms),
                    orderbook_venue("okx", 0.8, okx_price, ts_ms),
                ],
            });
        }

        assert!(
            !strategy.pricing.realized_vol_by_venue.contains_key("okx"),
            "non-eligible venues should not warm in the background"
        );

        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms: 4_000,
            topic: "platform.reference.test.chainlink".to_string(),
            fair_value: Some(3_103.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("chainlink", 1.0, 3_103.0, 4_000),
                orderbook_venue("okx", 0.8, 3_103.0, 4_000),
            ],
        });

        assert_eq!(
            strategy.pricing.fast_spot,
            Some(fast_spot("okx", 3_103.0, 4_000))
        );
        assert!(
            strategy.current_realized_vol_at(4_000).is_none(),
            "a venue that was previously ineligible should still cold-start when it first becomes eligible"
        );
    }

    #[test]
    fn realized_vol_does_not_borrow_ready_state_from_a_different_venue() {
        let mut strategy = ready_to_trade_strategy();
        strategy.config.vol_min_observations = 2;
        strategy.pricing = PricingState::from_config(&strategy.config);

        for (ts_ms, fair_value, fast_price) in [
            (1_000, 3_100.0, 3_100.0),
            (2_000, 3_101.0, 3_101.0),
            (3_000, 3_102.0, 3_102.0),
        ] {
            strategy.observe_reference_snapshot(&ReferenceSnapshot {
                ts_ms,
                topic: "platform.reference.test.chainlink".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("chainlink", 1.0, fair_value, ts_ms),
                    orderbook_venue("bybit", 0.9, fast_price, ts_ms),
                ],
            });
        }

        assert!(
            strategy.current_realized_vol_at(3_000).is_some(),
            "bybit should be warmed before the lead venue changes"
        );

        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms: 3_100,
            topic: "platform.reference.test.chainlink".to_string(),
            fair_value: Some(3_102.2),
            confidence: 1.0,
            venues: vec![
                oracle_venue("chainlink", 1.0, 3_102.2, 3_100),
                orderbook_venue("okx", 0.9, 3_102.2, 3_100),
            ],
        });

        assert_eq!(
            strategy.pricing.fast_spot,
            Some(fast_spot("okx", 3_102.2, 3_100))
        );
        assert!(
            strategy.current_realized_vol_at(3_100).is_none(),
            "selected venue should not inherit warmed vol from another venue"
        );
    }

    #[test]
    fn realized_vol_resets_per_venue_after_gap_even_if_other_venue_keeps_warming() {
        let mut strategy = ready_to_trade_strategy();
        strategy.config.vol_min_observations = 1;
        strategy.config.vol_gap_reset_secs = 1;
        strategy.config.vol_bridge_valid_secs = 10;
        strategy.config.lead_jitter_max_ms = 10_000;
        strategy.pricing = PricingState::from_config(&strategy.config);

        for (ts_ms, venue_name, fair_value, fast_price) in [
            (1_000, "bybit", 3_100.0, 3_100.0),
            (1_500, "bybit", 3_101.0, 3_101.0),
            (2_600, "okx", 3_101.5, 3_101.5),
            (3_100, "okx", 3_102.0, 3_102.0),
        ] {
            strategy.observe_reference_snapshot(&ReferenceSnapshot {
                ts_ms,
                topic: "platform.reference.test.chainlink".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("chainlink", 1.0, fair_value, ts_ms),
                    orderbook_venue(venue_name, 0.9, fast_price, ts_ms),
                ],
            });
        }

        assert_eq!(
            strategy.pricing.fast_spot,
            Some(fast_spot("okx", 3_102.0, 3_100))
        );
        assert!(
            strategy.current_realized_vol_at(3_100).is_some(),
            "okx should warm independently while bybit is absent"
        );

        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms: 4_201,
            topic: "platform.reference.test.chainlink".to_string(),
            fair_value: Some(3_102.5),
            confidence: 1.0,
            venues: vec![
                oracle_venue("chainlink", 1.0, 3_102.5, 4_201),
                orderbook_venue("bybit", 0.9, 3_102.5, 4_201),
            ],
        });

        assert_eq!(
            strategy.pricing.fast_spot,
            Some(fast_spot("bybit", 3_102.5, 4_201))
        );
        assert!(
            strategy.current_realized_vol_at(4_201).is_none(),
            "bybit should reset after its own gap instead of bridging stale or other-venue vol"
        );
    }

    #[test]
    fn pricing_state_reports_realized_vol_source_during_bridge_without_fast_spot() {
        let config = test_strategy().config.clone();
        let mut pricing = PricingState::from_config(&config);
        pricing.realized_vol_source_venue = Some("bybit".to_string());
        pricing.realized_vol.last_ready_vol = Some(1.5);
        pricing.realized_vol.last_ready_ts_ms = Some(1_200);

        assert_eq!(
            pricing.current_realized_vol_source_at(1_300),
            (Some("bybit".to_string()), Some(1_200))
        );
        assert_eq!(pricing.current_realized_vol_source_at(12_201), (None, None));
    }

    #[test]
    fn entry_evaluation_log_fields_keep_realized_vol_source_when_fast_spot_is_unavailable() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.pricing.fast_spot = None;
        strategy.pricing.last_reference_fair_value = Some(3_101.0);
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
        strategy.pricing.realized_vol_source_venue = Some("bybit".to_string());

        let submission = strategy.entry_submission_decision_at(1_200);
        let fields = strategy.entry_evaluation_log_fields_at(1_200, &submission);

        assert_eq!(fields.spot_venue_name, None);
        assert_eq!(fields.spot_price, Some(3_101.0));
        assert_eq!(fields.realized_vol, Some(2.5));
        assert_eq!(fields.realized_vol_source_venue.as_deref(), Some("bybit"));
        assert_eq!(fields.realized_vol_source_ts_ms, Some(1_200));
    }

    #[test]
    fn fair_probability_helper_is_directional_and_fail_closed_on_invalid_inputs() {
        let above = compute_fair_probability_up(&FairProbabilityInputs {
            spot_price: 3_105.0,
            strike_price: 3_100.0,
            seconds_to_expiry: 60,
            realized_vol: 0.45,
            pricing_kurtosis: 0.0,
        })
        .expect("valid inputs should produce fair probability");
        let below = compute_fair_probability_up(&FairProbabilityInputs {
            spot_price: 3_095.0,
            strike_price: 3_100.0,
            seconds_to_expiry: 60,
            realized_vol: 0.45,
            pricing_kurtosis: 0.0,
        })
        .expect("valid inputs should produce fair probability");

        assert!(
            above > 0.5,
            "above-strike spot should imply >50% up probability"
        );
        assert!(
            below < 0.5,
            "below-strike spot should imply <50% up probability"
        );
        assert!(above > below);
        assert!(
            compute_fair_probability_up(&FairProbabilityInputs {
                spot_price: 3_100.0,
                strike_price: 3_100.0,
                seconds_to_expiry: 60,
                realized_vol: 0.0,
                pricing_kurtosis: 0.0,
            })
            .is_none()
        );
    }

    #[test]
    fn theta_scaler_helper_increases_near_expiry_and_can_be_disabled() {
        let start = compute_theta_scaler(&ThetaScalerInputs {
            seconds_to_expiry: 300,
            period_duration_secs: 300,
            theta_decay_factor: 1.5,
        })
        .expect("valid theta inputs should compute");
        let near_expiry = compute_theta_scaler(&ThetaScalerInputs {
            seconds_to_expiry: 30,
            period_duration_secs: 300,
            theta_decay_factor: 1.5,
        })
        .expect("valid theta inputs should compute");

        assert!((start - 1.0).abs() < 1e-9);
        assert!(near_expiry > start);
        assert_eq!(
            compute_theta_scaler(&ThetaScalerInputs {
                seconds_to_expiry: 30,
                period_duration_secs: 300,
                theta_decay_factor: 0.0,
            }),
            Some(1.0)
        );
        assert!(
            compute_theta_scaler(&ThetaScalerInputs {
                seconds_to_expiry: 30,
                period_duration_secs: 0,
                theta_decay_factor: 1.5,
            })
            .is_none()
        );
    }

    #[test]
    fn entry_evaluation_blocks_when_realized_vol_is_not_ready() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_101.0, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = None;
        strategy.pricing.realized_vol.last_ready_ts_ms = None;

        let decision = strategy.entry_evaluation_at(1_200);

        assert!(decision.gate.blocked_by.is_empty());
        assert_eq!(
            decision.pricing_blocked_by,
            vec![EntryPricingBlockReason::RealizedVolNotReady]
        );
    }

    #[test]
    fn live_fair_probability_is_computed_from_strategy_state_once_vol_warms() {
        let mut strategy = ready_to_trade_strategy();
        strategy.config.vol_min_observations = 3;
        strategy.pricing = PricingState::from_config(&strategy.config);

        for (ts_ms, fair_value, fast_spot_price) in [
            (1_000, 3_100.0, 3_100.0),
            (2_000, 3_101.0, 3_101.5),
            (3_000, 3_102.0, 3_103.0),
            (4_000, 3_103.0, 3_104.0),
        ] {
            strategy.observe_reference_snapshot(&ReferenceSnapshot {
                ts_ms,
                topic: "platform.reference.test.chainlink".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![orderbook_venue("bybit", 0.9, fast_spot_price, ts_ms)],
            });
        }

        let fair_probability = strategy
            .current_fair_probability_up_at(4_000)
            .expect("warmed pricing state should produce fair probability");
        assert!(fair_probability > 0.5);

        let decision = strategy.entry_evaluation_at(4_000);
        assert!(decision.pricing_blocked_by.is_empty());
    }

    #[test]
    fn live_scaled_min_edge_uses_theta_scaler_near_expiry() {
        let mut strategy = ready_to_trade_strategy();
        strategy.config.worst_case_ev_min_bps = 10;
        strategy.config.theta_decay_factor = 1.5;

        let early = strategy
            .current_scaled_min_edge_bps_at(1_000)
            .expect("theta-scaled threshold should compute");
        let late = strategy
            .current_scaled_min_edge_bps_at(591_000)
            .expect("theta-scaled threshold should compute");

        assert!((early - 10.0).abs() < 1e-9);
        assert!(late > early);
    }

    #[test]
    fn switch_resets_only_active_market_state() {
        let mut strategy = test_strategy();
        strategy.market_lifecycle.insert(
            "A".to_string(),
            MarketLifecycleLedger {
                cooldown_expires_at_ms: Some(123),
                churn_count: 2,
            },
        );
        set_blind_recovery(&mut strategy, BlindRecoveryReason::CacheProbeFailed);
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_100.5, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = Some(1.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
        {
            let active = &mut strategy.active;
            active.interval_open = Some(3_000.0);
            active.warmup_count = 7;
        }

        strategy.apply_selection_snapshot(active_snapshot("B"));

        assert_eq!(
            strategy.market_lifecycle.get("A"),
            Some(&MarketLifecycleLedger {
                cooldown_expires_at_ms: Some(123),
                churn_count: 2,
            })
        );
        assert!(strategy.exposure.is_recovering());
        let active = &strategy.active;
        assert_eq!(active.market_id.as_deref(), Some("B"));
        assert!(active.interval_open.is_none());
        assert_eq!(active.warmup_count, 0);
        assert!(!active.outcome_fees.up_ready);
        assert!(!active.outcome_fees.down_ready);
        assert_eq!(
            strategy.pricing.fast_spot,
            Some(fast_spot("bybit", 3_100.5, 1_200))
        );
        assert_eq!(strategy.pricing.realized_vol.last_ready_vol, Some(1.5));
        assert_eq!(strategy.pricing.realized_vol.last_ready_ts_ms, Some(1_200));
    }

    #[test]
    fn same_market_interval_rollover_preserves_reconstructed_books() {
        let mut strategy = ready_to_trade_strategy();
        let original_up_bid = strategy.active.books.up.best_bid;
        let original_down_ask = strategy.active.books.down.best_ask;

        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 301_000));

        assert_eq!(strategy.active.market_id.as_deref(), Some("MKT-1"));
        assert_eq!(strategy.active.interval_start_ms, Some(301_000));
        assert_eq!(strategy.active.books.up.best_bid, original_up_bid);
        assert_eq!(strategy.active.books.down.best_ask, original_down_ask);
        assert!(strategy.active.books.is_priced());
    }

    #[test]
    fn position_events_update_live_position_state() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let position_id = PositionId::from("P-001");

        strategy.on_position_opened(position_opened_event(
            instrument_id,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        ));

        assert!(strategy.managed_position().is_some());
        assert_eq!(
            managed_position_ref(&strategy).cloned(),
            Some(OpenPositionState {
                market_id: Some("MKT-1".to_string()),
                instrument_id,
                position_id,
                outcome_side: Some(OutcomeSide::Up),
                outcome_fees: strategy.active.outcome_fees.clone(),
                historical_entry_fee_bps: None,
                entry_order_side: OrderSide::Buy,
                side: PositionSide::Long,
                quantity: Quantity::new(10.0, 2),
                avg_px_open: 0.450,
                interval_open: Some(3_100.0),
                selection_published_at_ms: Some(1_000),
                seconds_to_expiry_at_selection: Some(300),
                book: strategy.active.books.up.clone(),
            })
        );

        let recovered_position = managed_position_ref(&strategy)
            .cloned()
            .expect("position should be managed before exit pending");
        set_exit_pending(
            &mut strategy,
            recovered_position,
            ClientOrderId::from("EXIT-001"),
            false,
            false,
            ManagedPositionOrigin::RecoveryBootstrap,
        );
        strategy.on_position_closed(position_closed_event(instrument_id, position_id));

        assert!(strategy.managed_position().is_none());
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
            Some(ClientOrderId::from("EXIT-001"))
        );
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.fill_received),
            Some(false)
        );
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.close_received),
            Some(true)
        );
        assert!(!strategy.exposure.is_recovering());
    }

    #[test]
    fn exit_fill_keeps_pending_exit_until_position_closed() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let position_id = PositionId::from("P-EXIT-001");
        let exit_client_order_id = ClientOrderId::from("EXIT-001");

        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            position_id,
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_exit_pending(
            &mut strategy,
            open_position,
            exit_client_order_id,
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );

        strategy
            .on_order_filled(&order_filled_event(
                exit_client_order_id,
                instrument_id,
                position_id,
            ))
            .expect("exit fill bookkeeping should succeed");

        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
            Some(exit_client_order_id)
        );
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.fill_received),
            Some(true)
        );
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.close_received),
            Some(false)
        );
        assert!(strategy.managed_position().is_some());

        strategy.on_position_closed(position_closed_event(instrument_id, position_id));

        assert!(strategy.managed_position().is_none());
        assert!(pending_exit_ref(&strategy).is_none());
    }

    #[test]
    fn unrelated_position_close_does_not_clear_pending_exit_before_fill() {
        let mut strategy = ready_to_trade_strategy();
        let tracked_instrument = strategy.active.books.up.instrument_id.unwrap();
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: tracked_instrument,
            position_id: PositionId::from("P-TRACKED"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_exit_pending(
            &mut strategy,
            open_position,
            ClientOrderId::from("EXIT-001"),
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );

        strategy.on_position_closed(position_closed_event(
            tracked_instrument,
            PositionId::from("P-OTHER"),
        ));

        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
            Some(ClientOrderId::from("EXIT-001"))
        );
        assert!(strategy.managed_position().is_some());
    }

    #[test]
    fn unrelated_position_close_does_not_clear_filled_pending_exit() {
        let mut strategy = ready_to_trade_strategy();
        let tracked_instrument = strategy.active.books.up.instrument_id.unwrap();
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: tracked_instrument,
            position_id: PositionId::from("P-TRACKED"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_exit_pending(
            &mut strategy,
            open_position,
            ClientOrderId::from("EXIT-001"),
            true,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );

        strategy.on_position_closed(position_closed_event(
            tracked_instrument,
            PositionId::from("P-OTHER"),
        ));

        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
            Some(ClientOrderId::from("EXIT-001"))
        );
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.fill_received),
            Some(true)
        );
        assert!(strategy.managed_position().is_some());
    }

    #[test]
    fn exit_pending_state_clears_on_cancel_reject_and_expire() {
        let instrument_id = polymarket_instrument_id("condition-MKT-1", "MKT-1-UP");
        let exit_client_order_id = ClientOrderId::from("EXIT-001");

        let mut canceled = ready_to_trade_strategy();
        let canceled_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            position_id: PositionId::from("P-CANCEL"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: canceled.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(1.0, 2),
            avg_px_open: 0.45,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: canceled.active.books.up.clone(),
        };
        set_exit_pending(
            &mut canceled,
            canceled_position,
            exit_client_order_id,
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );
        canceled
            .on_order_canceled(&order_canceled_event(exit_client_order_id, instrument_id))
            .expect("exit cancel bookkeeping should succeed");
        assert!(pending_exit_ref(&canceled).is_none());
        assert!(canceled.managed_position().is_some());

        let mut rejected = ready_to_trade_strategy();
        let rejected_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            position_id: PositionId::from("P-REJECT"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: rejected.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(1.0, 2),
            avg_px_open: 0.45,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: rejected.active.books.up.clone(),
        };
        set_exit_pending(
            &mut rejected,
            rejected_position,
            exit_client_order_id,
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );
        rejected.on_order_rejected(order_rejected_event(exit_client_order_id, instrument_id));
        assert!(pending_exit_ref(&rejected).is_none());
        assert!(rejected.managed_position().is_some());

        let mut expired = ready_to_trade_strategy();
        let expired_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            position_id: PositionId::from("P-EXPIRE"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: expired.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(1.0, 2),
            avg_px_open: 0.45,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: expired.active.books.up.clone(),
        };
        set_exit_pending(
            &mut expired,
            expired_position,
            exit_client_order_id,
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );
        expired.on_order_expired(order_expired_event(exit_client_order_id, instrument_id));
        assert!(pending_exit_ref(&expired).is_none());
        assert!(expired.managed_position().is_some());
    }

    #[test]
    fn filled_exit_pending_ignores_stale_cancel_until_position_close() {
        let instrument_id = polymarket_instrument_id("condition-MKT-1", "MKT-1-UP");
        let exit_client_order_id = ClientOrderId::from("EXIT-FILLED-CANCEL");

        let mut strategy = ready_to_trade_strategy();
        let position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            position_id: PositionId::from("P-FILLED-CANCEL"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(1.0, 2),
            avg_px_open: 0.45,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_exit_pending(
            &mut strategy,
            position,
            exit_client_order_id,
            true,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );

        strategy
            .on_order_canceled(&order_canceled_event(exit_client_order_id, instrument_id))
            .expect("stale cancel should not clear filled exit pending");
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
            Some(exit_client_order_id)
        );
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.fill_received),
            Some(true)
        );

        strategy.on_position_closed(position_closed_event(
            instrument_id,
            PositionId::from("P-FILLED-CANCEL"),
        ));
        assert!(pending_exit_ref(&strategy).is_none());
        assert!(strategy.managed_position().is_none());
    }

    #[test]
    fn filled_exit_pending_ignores_stale_reject() {
        let instrument_id = polymarket_instrument_id("condition-MKT-1", "MKT-1-UP");
        let exit_client_order_id = ClientOrderId::from("EXIT-FILLED-REJECT");

        let mut strategy = ready_to_trade_strategy();
        let position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            position_id: PositionId::from("P-FILLED-REJECT"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(1.0, 2),
            avg_px_open: 0.45,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_exit_pending(
            &mut strategy,
            position,
            exit_client_order_id,
            true,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );

        strategy.on_order_rejected(order_rejected_event(exit_client_order_id, instrument_id));
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
            Some(exit_client_order_id)
        );
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.fill_received),
            Some(true)
        );
    }

    #[test]
    fn filled_exit_pending_ignores_stale_expire() {
        let instrument_id = polymarket_instrument_id("condition-MKT-1", "MKT-1-UP");
        let exit_client_order_id = ClientOrderId::from("EXIT-FILLED-EXPIRE");

        let mut strategy = ready_to_trade_strategy();
        let position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            position_id: PositionId::from("P-FILLED-EXPIRE"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(1.0, 2),
            avg_px_open: 0.45,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_exit_pending(
            &mut strategy,
            position,
            exit_client_order_id,
            true,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );

        strategy.on_order_expired(order_expired_event(exit_client_order_id, instrument_id));
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.client_order_id),
            Some(exit_client_order_id)
        );
        assert_eq!(
            pending_exit_ref(&strategy).map(|pending| pending.fill_received),
            Some(true)
        );
    }

    #[test]
    fn down_entry_submission_price_uses_best_ask_as_long_entry_cost() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.active.books.down.best_bid = Some(0.40);
        strategy.active.books.down.best_ask = Some(0.41);
        assert_eq!(
            strategy.submission_entry_price(OutcomeSide::Down),
            Some(0.41)
        );
        assert_eq!(
            strategy.executable_entry_cost(OutcomeSide::Down),
            Some(0.41)
        );
    }

    #[test]
    fn numeric_token_position_semantics_do_not_guess_without_suffixes() {
        let down_instrument = InstrumentId::from("0xcondition-222.POLYMARKET");
        let up_instrument = InstrumentId::from("0xcondition-111.POLYMARKET");

        assert_eq!(
            infer_strategy_outcome_side(OrderSide::Buy, PositionSide::Long, down_instrument),
            None
        );
        assert_eq!(
            infer_strategy_outcome_side(OrderSide::Buy, PositionSide::Long, up_instrument),
            None
        );
    }

    #[test]
    fn book_impact_cap_is_derived_from_vwap_slippage_against_best_touch() {
        let instrument_id = polymarket_instrument_id("condition-MKT-1", "MKT-1-UP");
        let mut state = OutcomeBookState::from_instrument_id(instrument_id);
        state.update_from_deltas(&book_deltas(
            instrument_id,
            &[
                (BookAction::Add, OrderSide::Buy, 0.49, 10.0),
                (BookAction::Add, OrderSide::Sell, 0.50, 10.0),
                (BookAction::Add, OrderSide::Sell, 0.60, 10.0),
            ],
        ));

        let zero_bps = state
            .max_buy_execution_within_vwap_slippage_bps(0)
            .expect("best-touch-only size should exist");
        let one_hundred_bps = state
            .max_buy_execution_within_vwap_slippage_bps(100)
            .expect("partial next-level size should exist");
        let loose = state
            .max_buy_execution_within_vwap_slippage_bps(5_000)
            .expect("full displayed size should exist");

        assert_eq!(zero_bps.quantity, 10.0);
        assert!(one_hundred_bps.quantity > zero_bps.quantity);
        assert!(one_hundred_bps.quantity < loose.quantity);
        assert_eq!(loose.quantity, 20.0);
        assert!(one_hundred_bps.vwap_price > zero_bps.vwap_price);
    }

    #[test]
    fn book_impact_cap_config_changes_sizing_decision() {
        let instrument_id = polymarket_instrument_id("condition-MKT-1", "MKT-1-UP");

        let mut loose = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        loose.config.book_impact_cap_bps = 5_000;
        loose.active.books.up.update_from_deltas(&book_deltas(
            instrument_id,
            &[
                (BookAction::Add, OrderSide::Buy, 0.49, 10.0),
                (BookAction::Add, OrderSide::Sell, 0.50, 10.0),
                (BookAction::Add, OrderSide::Sell, 0.60, 10.0),
            ],
        ));

        let mut tight = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        tight.config.book_impact_cap_bps = 0;
        tight.active.books.up.update_from_deltas(&book_deltas(
            instrument_id,
            &[
                (BookAction::Add, OrderSide::Buy, 0.49, 10.0),
                (BookAction::Add, OrderSide::Sell, 0.50, 10.0),
                (BookAction::Add, OrderSide::Sell, 0.60, 10.0),
            ],
        ));

        let loose_cap = loose.visible_book_notional_cap_usdc(OutcomeSide::Up);
        let tight_cap = tight.visible_book_notional_cap_usdc(OutcomeSide::Up);

        assert!(
            loose_cap
                .zip(tight_cap)
                .is_some_and(|(loose_cap, tight_cap)| tight_cap < loose_cap),
            "tighter impact cap should reduce the derived notional cap"
        );
    }

    #[test]
    fn fill_arms_cooldown_for_filled_market_not_current_selection() {
        let mut strategy = ready_to_trade_strategy();
        let entry_client_order_id = ClientOrderId::from("ENTRY-A");
        let position_id = PositionId::from("P-A");
        let instrument_a = strategy.active.books.up.instrument_id.unwrap();
        let pending = pending_entry_state(
            &strategy,
            entry_client_order_id,
            instrument_a,
            OutcomeSide::Up,
            strategy.active.books.up.clone(),
        );
        set_pending_entry(&mut strategy, pending);

        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
        strategy
            .on_order_filled(&order_filled_event(
                entry_client_order_id,
                instrument_a,
                position_id,
            ))
            .expect("fill bookkeeping should succeed");

        assert!(strategy.market_in_cooldown("MKT-1", 1_000));
        assert!(!strategy.market_in_cooldown("MKT-2", 1_000));
        assert_eq!(strategy.market_churn_count("MKT-1"), 1);
        assert_eq!(strategy.market_churn_count("MKT-2"), 0);
    }

    #[test]
    fn exit_fill_arms_cooldown_for_position_market_not_current_selection() {
        let mut strategy = ready_to_trade_strategy();
        let tracked_instrument = strategy.active.books.up.instrument_id.unwrap();
        let exit_client_order_id = ClientOrderId::from("EXIT-A");
        let position_id = PositionId::from("P-A");
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: tracked_instrument,
            position_id,
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_exit_pending(
            &mut strategy,
            open_position,
            exit_client_order_id,
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));

        strategy
            .on_order_filled(&order_filled_event(
                exit_client_order_id,
                tracked_instrument,
                position_id,
            ))
            .expect("exit fill bookkeeping should succeed");

        assert!(strategy.market_in_cooldown("MKT-1", 1_000));
        assert!(!strategy.market_in_cooldown("MKT-2", 1_000));
        assert_eq!(strategy.market_churn_count("MKT-1"), 1);
        assert_eq!(strategy.market_churn_count("MKT-2"), 0);
    }

    #[test]
    fn exit_fill_without_known_position_market_does_not_cool_down_active_selection() {
        let mut strategy = ready_to_trade_strategy();
        let tracked_instrument = strategy.active.books.up.instrument_id.unwrap();
        let exit_client_order_id = ClientOrderId::from("EXIT-UNKNOWN");
        let position_id = PositionId::from("P-UNKNOWN");
        let open_position = OpenPositionState {
            market_id: None,
            instrument_id: tracked_instrument,
            position_id,
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_exit_pending(
            &mut strategy,
            open_position,
            exit_client_order_id,
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));

        strategy
            .on_order_filled(&order_filled_event(
                exit_client_order_id,
                tracked_instrument,
                position_id,
            ))
            .expect("exit fill bookkeeping should succeed");

        assert!(!strategy.market_in_cooldown("MKT-2", 1_000));
    }

    #[test]
    fn delayed_exit_fill_after_position_closed_does_not_cool_down_active_selection() {
        let mut strategy = ready_to_trade_strategy();
        let tracked_instrument = strategy.active.books.up.instrument_id.unwrap();
        let exit_client_order_id = ClientOrderId::from("EXIT-DELAYED");
        let position_id = PositionId::from("P-DELAYED");
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: tracked_instrument,
            position_id,
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_exit_pending(
            &mut strategy,
            open_position,
            exit_client_order_id,
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
        strategy.on_position_closed(position_closed_event(tracked_instrument, position_id));

        strategy
            .on_order_filled(&order_filled_event(
                exit_client_order_id,
                tracked_instrument,
                position_id,
            ))
            .expect("delayed exit fill should not arm the wrong market cooldown");

        assert!(strategy.market_in_cooldown("MKT-1", 1_000));
        assert!(!strategy.market_in_cooldown("MKT-2", 1_000));
        assert!(pending_exit_ref(&strategy).is_none());
    }

    #[test]
    fn rotated_position_uses_position_book_for_thin_book_forced_flat() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let position_instrument = InstrumentId::from("condition-MKT-A-UP.POLYMARKET");
        let mut tracked_book = OutcomeBookState::from_instrument_id(position_instrument);
        tracked_book.last_observed_instrument_id = Some(position_instrument);
        tracked_book.best_bid = Some(0.430);
        tracked_book.best_ask = Some(0.450);
        tracked_book.liquidity_available = Some(5.0);
        let open_position = OpenPositionState {
            market_id: Some("MKT-A".to_string()),
            instrument_id: position_instrument,
            position_id: PositionId::from("P-THIN-001"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(5.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: tracked_book,
        };
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
        strategy.active.books.up.liquidity_available = Some(5_000.0);
        strategy.active.books.down.liquidity_available = Some(5_000.0);

        let decision = strategy.exit_submission_decision_at(2_000);

        assert!(
            decision
                .forced_flat_reasons
                .contains(&ForcedFlatReason::ThinBook)
        );
        assert_eq!(decision.order_side, Some(OrderSide::Sell));
        assert_eq!(decision.instrument_id, Some(position_instrument));
    }

    #[test]
    fn untracked_position_close_keeps_recovery_fail_closed() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        set_blind_recovery(&mut strategy, BlindRecoveryReason::CacheProbeFailed);

        strategy.on_position_closed(position_closed_event(
            instrument_id,
            PositionId::from("P-X"),
        ));

        assert!(strategy.exposure.is_recovering());
    }

    #[test]
    fn fill_after_rotation_preserves_exitable_position_book_and_subscription() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_a = strategy.active.books.up.instrument_id.unwrap();
        let entry_client_order_id = ClientOrderId::from("ENTRY-A");
        let position_id = PositionId::from("P-A");
        let original_book = strategy.active.books.up.clone();
        let pending = pending_entry_state(
            &strategy,
            entry_client_order_id,
            instrument_a,
            OutcomeSide::Up,
            original_book.clone(),
        );
        set_pending_entry(&mut strategy, pending);

        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
        strategy
            .on_order_filled(&order_filled_event(
                entry_client_order_id,
                instrument_a,
                position_id,
            ))
            .expect("fill bookkeeping should succeed");

        assert_eq!(
            managed_position_ref(&strategy).and_then(|p| p.book.best_bid),
            original_book.best_bid
        );
        assert_eq!(
            managed_position_ref(&strategy).and_then(|p| p.interval_open),
            Some(3_100.0)
        );
        assert_eq!(
            managed_position_ref(&strategy).and_then(|p| p.selection_published_at_ms),
            Some(1_000)
        );
        assert_eq!(
            managed_position_ref(&strategy).and_then(|p| p.seconds_to_expiry_at_selection),
            Some(300)
        );
        assert_eq!(
            managed_position_ref(&strategy).and_then(|p| p.outcome_fees.up_token_id.as_deref()),
            Some("MKT-1-UP")
        );
        assert_eq!(
            strategy.book_subscriptions.tracked_position_instrument_id,
            Some(instrument_a)
        );
        let decision = strategy.exit_submission_decision_at(2_000);
        assert_eq!(decision.instrument_id, Some(instrument_a));
        assert_eq!(decision.order_side, Some(OrderSide::Sell));
    }

    #[test]
    fn entry_fill_without_position_id_stays_fail_closed_until_position_event_arrives() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let entry_client_order_id = ClientOrderId::from("ENTRY-NO-POS");
        let original_book = strategy.active.books.up.clone();
        let pending = pending_entry_state(
            &strategy,
            entry_client_order_id,
            instrument_id,
            OutcomeSide::Up,
            original_book.clone(),
        );
        set_pending_entry(&mut strategy, pending);

        strategy
            .on_order_filled(&order_filled_event_with_details(
                entry_client_order_id,
                instrument_id,
                None,
                OrderSide::Buy,
            ))
            .expect("fill without position id should not wedge");

        assert!(strategy.exposure.is_recovering());
        assert!(strategy.managed_position().is_none());
        assert_eq!(
            strategy
                .pending_entry()
                .map(|pending| pending.client_order_id),
            Some(entry_client_order_id)
        );
        assert!(strategy.market_in_cooldown("MKT-1", 1_000));

        strategy.on_position_opened(position_opened_event(
            instrument_id,
            PositionId::from("P-LATE"),
            Quantity::new(10.0, 2),
            0.450,
        ));

        assert!(strategy.managed_position().is_some());
        assert_eq!(
            managed_position_ref(&strategy).map(|position| position.position_id),
            Some(PositionId::from("P-LATE"))
        );
        assert_eq!(
            managed_position_ref(&strategy).and_then(|position| position.market_id.as_deref()),
            Some("MKT-1")
        );
        assert_eq!(
            managed_position_ref(&strategy).map(|position| position.book.clone()),
            Some(original_book)
        );
        assert!(strategy.pending_entry().is_none());
    }

    #[test]
    fn late_entry_terminal_events_preserve_entry_reconcile_fail_closed_state() {
        let entry_client_order_id = ClientOrderId::from("ENTRY-LATE-TERM");
        let instrument_id = polymarket_instrument_id("condition-MKT-1", "MKT-1-UP");

        let mut canceled = ready_to_trade_strategy();
        let canceled_pending = pending_entry_state(
            &canceled,
            entry_client_order_id,
            instrument_id,
            OutcomeSide::Up,
            canceled.active.books.up.clone(),
        );
        set_entry_reconcile_pending(
            &mut canceled,
            canceled_pending,
            EntryReconcileReason::AwaitingPositionMaterialization,
        );
        canceled
            .on_order_canceled(&order_canceled_event(entry_client_order_id, instrument_id))
            .expect("late cancel should preserve fail-closed reconcile state");
        assert!(matches!(
            canceled.exposure,
            ExposureState::EntryReconcilePending { .. }
        ));

        let mut rejected = ready_to_trade_strategy();
        let rejected_pending = pending_entry_state(
            &rejected,
            entry_client_order_id,
            instrument_id,
            OutcomeSide::Up,
            rejected.active.books.up.clone(),
        );
        set_entry_reconcile_pending(
            &mut rejected,
            rejected_pending,
            EntryReconcileReason::AwaitingPositionMaterialization,
        );
        rejected.on_order_rejected(order_rejected_event(entry_client_order_id, instrument_id));
        assert!(matches!(
            rejected.exposure,
            ExposureState::EntryReconcilePending { .. }
        ));

        let mut expired = ready_to_trade_strategy();
        let expired_pending = pending_entry_state(
            &expired,
            entry_client_order_id,
            instrument_id,
            OutcomeSide::Up,
            expired.active.books.up.clone(),
        );
        set_entry_reconcile_pending(
            &mut expired,
            expired_pending,
            EntryReconcileReason::AwaitingPositionMaterialization,
        );
        expired.on_order_expired(order_expired_event(entry_client_order_id, instrument_id));
        assert!(matches!(
            expired.exposure,
            ExposureState::EntryReconcilePending { .. }
        ));
    }

    #[test]
    fn sell_fill_enters_recovery_without_materializing_position() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.down.instrument_id.unwrap();
        let entry_client_order_id = ClientOrderId::from("ENTRY-SELL");
        let pending = pending_entry_state(
            &strategy,
            entry_client_order_id,
            instrument_id,
            OutcomeSide::Down,
            strategy.active.books.down.clone(),
        );
        set_pending_entry(&mut strategy, pending);

        strategy
            .on_order_filled(&order_filled_event_with_details(
                entry_client_order_id,
                instrument_id,
                Some(PositionId::from("P-SHORT")),
                OrderSide::Sell,
            ))
            .expect("sell fill should fail closed into recovery");

        assert!(strategy.exposure.is_recovering());
        assert!(strategy.managed_position().is_none());
        assert_eq!(
            strategy
                .pending_entry()
                .map(|pending| pending.client_order_id),
            Some(entry_client_order_id)
        );
        assert_eq!(
            strategy
                .pending_entry()
                .map(|pending| pending.instrument_id),
            Some(instrument_id)
        );
    }

    #[test]
    fn pending_entry_short_position_event_stays_fail_closed_without_materializing_position() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.down.instrument_id.unwrap();
        let entry_client_order_id = ClientOrderId::from("ENTRY-SELL");
        let pending = pending_entry_state(
            &strategy,
            entry_client_order_id,
            instrument_id,
            OutcomeSide::Down,
            strategy.active.books.down.clone(),
        );
        set_pending_entry(&mut strategy, pending);

        strategy.on_position_opened(position_opened_event_with_details(
            instrument_id,
            PositionId::from("P-SHORT"),
            Quantity::new(10.0, 2),
            0.450,
            OrderSide::Sell,
            PositionSide::Short,
        ));

        assert!(strategy.exposure.is_recovering());
        assert!(strategy.managed_position().is_none());
        let quarantined = match &strategy.exposure {
            ExposureState::UnsupportedObserved(state) => state,
            other => panic!("expected unsupported observed exposure, got {other:?}"),
        };
        assert_eq!(quarantined.observed.instrument_id, instrument_id);
        assert_eq!(
            quarantined.observed.position_id,
            PositionId::from("P-SHORT")
        );
        assert_eq!(quarantined.observed.entry_order_side, OrderSide::Sell);
        assert_eq!(quarantined.observed.side, PositionSide::Short);
        assert!(strategy.pending_entry().is_none());
    }

    #[test]
    fn pending_entry_unknown_position_side_stays_fail_closed_without_materializing_position() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let entry_client_order_id = ClientOrderId::from("ENTRY-BAD-SIDE");
        let pending = pending_entry_state(
            &strategy,
            entry_client_order_id,
            instrument_id,
            OutcomeSide::Up,
            strategy.active.books.up.clone(),
        );
        set_pending_entry(&mut strategy, pending);

        strategy.on_position_opened(position_opened_event_with_details(
            instrument_id,
            PositionId::from("P-BAD-SIDE"),
            Quantity::new(10.0, 2),
            0.450,
            OrderSide::Buy,
            PositionSide::Flat,
        ));

        assert!(strategy.exposure.is_recovering());
        assert!(strategy.managed_position().is_none());
        assert_eq!(
            strategy
                .pending_entry()
                .map(|pending| pending.client_order_id),
            Some(entry_client_order_id)
        );
    }

    #[test]
    fn position_opened_after_rotation_preserves_existing_position_context() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_a = strategy.active.books.up.instrument_id.unwrap();
        let preserved_book = strategy.active.books.up.clone();
        let preserved_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: instrument_a,
            position_id: PositionId::from("P-A"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: preserved_book.clone(),
        };
        set_managed_position(
            &mut strategy,
            preserved_position,
            ManagedPositionOrigin::StrategyEntry,
        );

        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
        strategy.active.interval_open = Some(3_200.0);
        strategy.on_position_opened(position_opened_event(
            instrument_a,
            PositionId::from("P-A"),
            Quantity::new(10.0, 2),
            0.450,
        ));

        let open_position = managed_position_ref(&strategy)
            .cloned()
            .expect("position should remain tracked");
        assert_eq!(open_position.market_id.as_deref(), Some("MKT-1"));
        assert_eq!(open_position.interval_open, Some(3_100.0));
        assert_eq!(open_position.selection_published_at_ms, Some(1_000));
        assert_eq!(open_position.seconds_to_expiry_at_selection, Some(300));
        assert_eq!(
            open_position.outcome_fees.up_token_id.as_deref(),
            Some("MKT-1-UP")
        );
        assert_eq!(open_position.book.best_bid, preserved_book.best_bid);
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
    fn interval_open_uses_raw_chainlink_price_not_fused_reference_value() {
        let mut strategy = test_strategy();
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.chainlink".to_string(),
            fair_value: Some(3_107.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("chainlink", 1.0, 3_100.0, 1_000),
                orderbook_venue("bybit", 0.9, 3_120.0, 1_000),
            ],
        });

        assert_eq!(strategy.active.interval_open, Some(3_100.0));
    }

    #[test]
    fn interval_open_prefers_polymarket_price_to_beat_over_chainlink() {
        let mut strategy = test_strategy();
        let mut snapshot = active_snapshot_with_start("MKT-1", 1_000);
        let SelectionState::Active { market } = &mut snapshot.decision.state else {
            panic!("expected active snapshot");
        };
        market.price_to_beat = Some(3_099.0);
        strategy.apply_selection_snapshot(snapshot);

        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.chainlink".to_string(),
            fair_value: Some(3_107.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("chainlink", 1.0, 3_100.0, 1_000),
                orderbook_venue("bybit", 0.9, 3_120.0, 1_000),
            ],
        });

        assert_eq!(strategy.active.interval_open, Some(3_099.0));
    }

    #[test]
    fn interval_open_falls_back_to_fused_reference_when_no_polymarket_or_oracle_anchor_exists() {
        let mut strategy = test_strategy();
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.chainlink".to_string(),
            fair_value: Some(3_107.0),
            confidence: 1.0,
            venues: vec![],
        });

        assert_eq!(strategy.active.interval_open, Some(3_107.0));
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

    #[tokio::test(flavor = "current_thread")]
    async fn same_market_new_interval_rollover_warms_fees_again() {
        let fee_provider = RecordingFeeProvider::cold();
        let mut strategy = test_strategy_with_fee_provider(fee_provider.clone());

        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
        tokio::task::yield_now().await;
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 2_000));
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
        assert_eq!(
            active.outcome_fees.down_token_id.as_deref(),
            Some("MKT-1-DOWN")
        );
    }

    #[test]
    fn freeze_continues_reference_preparation_without_opening_entries() {
        let mut strategy = test_strategy();
        strategy.config.warmup_tick_count = 2;
        strategy.apply_selection_snapshot(freeze_snapshot_with_start("MKT-1", 1_000));

        strategy.observe_reference_snapshot(&reference_tick(900, 3_099.0));
        assert!(strategy.active.interval_open.is_none());
        assert_eq!(strategy.active.last_reference_ts_ms, None);
        assert_eq!(strategy.active.warmup_count, 0);

        strategy.observe_reference_snapshot(&reference_tick(1_000, 3_100.0));
        assert_eq!(strategy.active.interval_open, Some(3_100.0));
        assert_eq!(strategy.active.last_reference_ts_ms, Some(1_000));
        assert_eq!(strategy.active.warmup_count, 1);
        assert!(!strategy.active.warmup_complete());
        assert!(strategy.active.forced_flat);

        strategy.observe_reference_snapshot(&reference_tick(1_100, 3_101.0));
        assert_eq!(strategy.active.last_reference_ts_ms, Some(1_100));
        assert_eq!(strategy.active.warmup_count, 2);
        assert!(strategy.active.warmup_complete());
        assert!(strategy.active.forced_flat);
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
    fn switch_with_cached_fee_rates_stays_ready_while_refresh_runs() {
        let fee_provider = RecordingFeeProvider::cold();
        fee_provider.set_fee("MKT-2-UP", Decimal::new(175, 2));
        fee_provider.set_fee("MKT-2-DOWN", Decimal::new(180, 2));
        let mut strategy = test_strategy_with_fee_provider(fee_provider);

        strategy.apply_selection_snapshot(active_snapshot("MKT-1"));
        strategy.apply_selection_snapshot(active_snapshot("MKT-2"));

        assert!(strategy.active.outcome_fees.up_ready);
        assert!(strategy.active.outcome_fees.down_ready);
    }

    #[test]
    fn same_market_new_interval_with_cached_fee_rates_stays_ready_while_refresh_runs() {
        let fee_provider = RecordingFeeProvider::cold();
        fee_provider.set_fee("MKT-1-UP", Decimal::new(175, 2));
        fee_provider.set_fee("MKT-1-DOWN", Decimal::new(180, 2));
        let mut strategy = test_strategy_with_fee_provider(fee_provider);

        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
        assert!(strategy.active.outcome_fees.market_ready());

        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 2_000));

        assert!(strategy.active.outcome_fees.up_ready);
        assert!(strategy.active.outcome_fees.down_ready);
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
                BookSubscriptionEvent::unsubscribe(InstrumentId::from(
                    "condition-A-A-UP.POLYMARKET"
                )),
                BookSubscriptionEvent::unsubscribe(InstrumentId::from(
                    "condition-A-A-DOWN.POLYMARKET"
                )),
                BookSubscriptionEvent::subscribe(InstrumentId::from("condition-B-B-UP.POLYMARKET")),
                BookSubscriptionEvent::subscribe(InstrumentId::from(
                    "condition-B-B-DOWN.POLYMARKET"
                )),
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

        msgbus::publish_any(
            runtime_selection_topic(&StrategyId::from("ETHCHAINLINKTAKER-001")).into(),
            &active_snapshot("B"),
        );

        let actor_id = unsafe { (&*actor_rc.get()).actor_id().inner() };
        let actor_ref = get_actor_unchecked::<EthChainlinkTaker>(&actor_id);
        assert_eq!(actor_ref.active.market_id.as_deref(), Some("B"));
        assert_eq!(
            actor_ref.book_subscription_events,
            vec![
                BookSubscriptionEvent::unsubscribe(InstrumentId::from(
                    "condition-A-A-UP.POLYMARKET"
                )),
                BookSubscriptionEvent::unsubscribe(InstrumentId::from(
                    "condition-A-A-DOWN.POLYMARKET"
                )),
                BookSubscriptionEvent::subscribe(InstrumentId::from("condition-B-B-UP.POLYMARKET")),
                BookSubscriptionEvent::subscribe(InstrumentId::from(
                    "condition-B-B-DOWN.POLYMARKET"
                )),
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

    #[test]
    fn task4_lead_arbitration_uses_composite_score_over_fixed_precedence() {
        let candidates = vec![
            lead_signal("younger_but_weaker", 10, 10, 0.81, 1.0, 0.01),
            lead_signal("older_but_stronger", 20, 10, 0.99, 4.0, 0.01),
        ];

        let selected =
            arbitrate_lead_reference(&candidates, 0.80, 25).expect("winner should be eligible");

        assert_eq!(selected.venue_name, "older_but_stronger");
    }

    #[test]
    fn task4_lead_arbitration_falls_back_to_chainlink_when_no_fast_venue_is_eligible() {
        let candidates = vec![
            lead_signal("too_noisy", 20, 300, 0.95, 4.0, 0.01),
            lead_signal("disagrees", 20, 20, 0.79, 4.0, 0.01),
            lead_signal("weightless", 20, 20, 0.95, 0.0, 0.01),
        ];

        let selected = arbitrate_lead_reference(&candidates, 0.80, 250);

        assert!(selected.is_none());
    }

    #[test]
    fn task4_lead_arbitration_fails_closed_on_exact_score_tie() {
        let candidates = vec![
            lead_signal("lighter", 10, 10, 0.90, 2.0, 0.01),
            lead_signal("heavier", 10, 10, 0.90, 3.0, 0.01),
        ];

        let selected = arbitrate_lead_reference(&candidates, 0.80, 25);

        assert!(selected.is_none());
    }

    #[test]
    fn task4_uncertainty_band_grows_with_jitter_and_time_to_resolution() {
        let narrow = uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability: 0.01,
            jitter_penalty_probability: 0.002,
            time_uncertainty_probability: 0.003,
            fee_uncertainty_probability: 0.0,
        })
        .expect("valid uncertainty inputs should produce a band");
        let wider_from_jitter = uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability: 0.01,
            jitter_penalty_probability: 0.004,
            time_uncertainty_probability: 0.003,
            fee_uncertainty_probability: 0.0,
        })
        .expect("valid uncertainty inputs should produce a band");
        let wider_from_time = uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability: 0.01,
            jitter_penalty_probability: 0.002,
            time_uncertainty_probability: 0.005,
            fee_uncertainty_probability: 0.0,
        })
        .expect("valid uncertainty inputs should produce a band");

        assert!(wider_from_jitter > narrow);
        assert!(wider_from_time > narrow);
    }

    #[test]
    fn task4_uncertainty_band_grows_with_fee_uncertainty() {
        let narrow = uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability: 0.01,
            jitter_penalty_probability: 0.002,
            time_uncertainty_probability: 0.003,
            fee_uncertainty_probability: 0.0,
        })
        .expect("valid uncertainty inputs should produce a band");
        let wide = uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability: 0.01,
            jitter_penalty_probability: 0.002,
            time_uncertainty_probability: 0.003,
            fee_uncertainty_probability: 0.02,
        })
        .expect("valid uncertainty inputs should produce a band");

        assert!(wide > narrow);
    }

    #[test]
    fn task4_uncertainty_band_fails_closed_on_invalid_component() {
        assert_eq!(
            uncertainty_band_probability(&UncertaintyBandInputs {
                lead_gap_probability: f64::NAN,
                jitter_penalty_probability: 0.002,
                time_uncertainty_probability: 0.003,
                fee_uncertainty_probability: 0.0,
            }),
            None
        );
        assert_eq!(
            uncertainty_band_probability(&UncertaintyBandInputs {
                lead_gap_probability: 1.2,
                jitter_penalty_probability: 0.002,
                time_uncertainty_probability: 0.003,
                fee_uncertainty_probability: 0.0,
            }),
            None
        );
        assert_eq!(
            uncertainty_band_probability(&UncertaintyBandInputs {
                lead_gap_probability: 0.40,
                jitter_penalty_probability: 0.30,
                time_uncertainty_probability: 0.20,
                fee_uncertainty_probability: 0.20,
            }),
            None
        );
    }

    #[test]
    fn task4_worst_case_ev_uses_side_specific_bounds_and_fees_fail_closed() {
        let up_zero_fee = compute_worst_case_ev_bps(
            OutcomeSide::Up,
            &WorstCaseEvInputs {
                fair_probability: Some(0.60),
                uncertainty_band_probability: 0.05,
                executable_entry_cost: 0.50,
                fee_bps: Some(0.0),
            },
        )
        .expect("up zero-fee EV should be computable");
        let up_paid_fee = compute_worst_case_ev_bps(
            OutcomeSide::Up,
            &WorstCaseEvInputs {
                fair_probability: Some(0.60),
                uncertainty_band_probability: 0.05,
                executable_entry_cost: 0.50,
                fee_bps: Some(200.0),
            },
        )
        .expect("up paid-fee EV should be computable");
        let down_zero_fee = compute_worst_case_ev_bps(
            OutcomeSide::Down,
            &WorstCaseEvInputs {
                fair_probability: Some(0.60),
                uncertainty_band_probability: 0.05,
                executable_entry_cost: 0.50,
                fee_bps: Some(0.0),
            },
        )
        .expect("down zero-fee EV should be computable");

        assert!(up_paid_fee < up_zero_fee);
        assert!(up_zero_fee > down_zero_fee);
        assert_eq!(
            compute_worst_case_ev_bps(
                OutcomeSide::Up,
                &WorstCaseEvInputs {
                    fair_probability: Some(0.60),
                    uncertainty_band_probability: 0.05,
                    executable_entry_cost: 0.50,
                    fee_bps: None,
                },
            ),
            None
        );
        assert_eq!(
            compute_worst_case_ev_bps(
                OutcomeSide::Up,
                &WorstCaseEvInputs {
                    fair_probability: Some(1.2),
                    uncertainty_band_probability: 0.05,
                    executable_entry_cost: 0.50,
                    fee_bps: Some(0.0),
                },
            ),
            None
        );
        assert_eq!(
            compute_worst_case_ev_bps(
                OutcomeSide::Up,
                &WorstCaseEvInputs {
                    fair_probability: Some(0.60),
                    uncertainty_band_probability: 1.5,
                    executable_entry_cost: 0.50,
                    fee_bps: Some(0.0),
                },
            ),
            None
        );
    }

    #[test]
    fn task4_side_selection_picks_higher_worst_case_ev_when_both_clear_threshold() {
        let side = choose_entry_side(&SideSelectionInputs {
            up_worst_ev_bps: Some(9.0),
            down_worst_ev_bps: Some(11.0),
            min_worst_case_ev_bps: 8.0,
        });

        assert_eq!(side, Some(OutcomeSide::Down));
    }

    #[test]
    fn task4_side_selection_requires_strictly_greater_than_threshold() {
        let side = choose_entry_side(&SideSelectionInputs {
            up_worst_ev_bps: Some(8.0),
            down_worst_ev_bps: Some(7.0),
            min_worst_case_ev_bps: 8.0,
        });

        assert_eq!(side, None);
    }

    #[test]
    fn task4_side_selection_fails_closed_on_missing_or_invalid_side_ev() {
        assert_eq!(
            choose_entry_side(&SideSelectionInputs {
                up_worst_ev_bps: Some(9.0),
                down_worst_ev_bps: None,
                min_worst_case_ev_bps: 8.0,
            }),
            None
        );
        assert_eq!(
            choose_entry_side(&SideSelectionInputs {
                up_worst_ev_bps: Some(f64::NAN),
                down_worst_ev_bps: Some(9.0),
                min_worst_case_ev_bps: 8.0,
            }),
            None
        );
    }

    #[test]
    fn task4_side_selection_fails_closed_on_equal_positive_evs() {
        let side = choose_entry_side(&SideSelectionInputs {
            up_worst_ev_bps: Some(9.0),
            down_worst_ev_bps: Some(9.0),
            min_worst_case_ev_bps: 8.0,
        });

        assert_eq!(side, None);
    }

    #[test]
    fn task4_robust_sizing_shrinks_with_risk_and_respects_caps() {
        let low_risk = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_usdc: 2.0,
            risk_lambda: 0.1,
            max_position_usdc: 100.0,
            impact_cap_usdc: 100.0,
        });
        let high_risk = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_usdc: 2.0,
            risk_lambda: 2.0,
            max_position_usdc: 100.0,
            impact_cap_usdc: 100.0,
        });
        let capped = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_usdc: 2.0,
            risk_lambda: 0.1,
            max_position_usdc: 12.0,
            impact_cap_usdc: 7.5,
        });

        assert!(high_risk < low_risk);
        assert_eq!(capped, 7.5);
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_usdc: 0.0,
                risk_lambda: 0.1,
                max_position_usdc: 100.0,
                impact_cap_usdc: 100.0,
            }),
            0.0
        );
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_usdc: 2.0,
                risk_lambda: 0.0,
                max_position_usdc: 100.0,
                impact_cap_usdc: 100.0,
            }),
            100.0
        );
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_usdc: 2.0,
                risk_lambda: -0.1,
                max_position_usdc: 100.0,
                impact_cap_usdc: 100.0,
            }),
            0.0
        );
    }

    #[test]
    fn task5_entry_gate_reports_all_frozen_block_reasons_explicitly() {
        let mut strategy = test_strategy();
        strategy.apply_selection_snapshot(freeze_snapshot_with_start("MKT-1", 1_000));
        strategy.market_lifecycle.insert(
            "MKT-1".to_string(),
            MarketLifecycleLedger {
                cooldown_expires_at_ms: Some(5_000),
                churn_count: 0,
            },
        );
        let pending = PendingEntryState {
            client_order_id: ClientOrderId::from("ENTRY-001"),
            market_id: Some("MKT-1".to_string()),
            instrument_id: strategy.active.books.up.instrument_id.unwrap(),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            interval_open: None,
            selection_published_at_ms: None,
            seconds_to_expiry_at_selection: None,
            book: strategy.active.books.up.clone(),
        };
        set_entry_reconcile_pending(
            &mut strategy,
            pending,
            EntryReconcileReason::AwaitingPositionMaterialization,
        );

        let decision = strategy.entry_gate_decision_at(2_000);

        assert_eq!(
            decision.blocked_by,
            vec![
                EntryBlockReason::PhaseNotActive,
                EntryBlockReason::MetadataMismatch,
                EntryBlockReason::ActiveBookNotPriced,
                EntryBlockReason::IntervalOpenMissing,
                EntryBlockReason::WarmupIncomplete,
                EntryBlockReason::FeesNotReady,
                EntryBlockReason::RecoveryMode,
                EntryBlockReason::MarketCoolingDown,
                EntryBlockReason::ForcedFlat(ForcedFlatReason::Freeze),
                EntryBlockReason::OnePositionInvariant(ExposureOccupancy::EntryReconcilePending),
            ]
        );
    }

    #[test]
    fn task5_one_position_invariant_panics_in_debug_or_rejects_in_release() {
        let mut strategy = ready_to_trade_strategy();
        let invariant_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: strategy.active.books.up.instrument_id.unwrap(),
            position_id: PositionId::from("P-INVARIANT-1"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(5.0, 2),
            avg_px_open: 0.45,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_exit_pending(
            &mut strategy,
            invariant_position,
            ClientOrderId::from("EXIT-001"),
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            strategy.enforce_one_position_invariant()
        }));

        if cfg!(debug_assertions) {
            assert!(result.is_err());
        } else {
            assert!(result.expect("release builds should not panic").is_err());
        }
    }

    #[test]
    fn entry_gate_reports_one_position_invariant_only_on_occupancy_change() {
        let mut strategy = ready_to_trade_strategy();
        let invariant_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: strategy.active.books.up.instrument_id.unwrap(),
            position_id: PositionId::from("P-INVARIANT-2"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(5.0, 2),
            avg_px_open: 0.45,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_exit_pending(
            &mut strategy,
            invariant_position,
            ClientOrderId::from("EXIT-001"),
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );

        let first = strategy.entry_gate_decision_at(2_000);
        let second = strategy.entry_gate_decision_at(2_001);

        assert!(
            first
                .blocked_by
                .contains(&EntryBlockReason::OnePositionInvariant(
                    ExposureOccupancy::ExitPending
                ))
        );
        assert_eq!(strategy.last_reported_exposure_occupancy.get(), None);
        assert_eq!(first.blocked_by, second.blocked_by);

        strategy.exposure = ExposureState::Flat;
        let cleared = strategy.entry_gate_decision_at(2_002);
        assert!(
            !cleared
                .blocked_by
                .contains(&EntryBlockReason::OnePositionInvariant(
                    ExposureOccupancy::ExitPending
                ))
        );
        assert_eq!(strategy.last_reported_exposure_occupancy.get(), None);
    }

    #[test]
    fn entry_gate_reports_only_unexpected_occupancies_as_invariant_violations() {
        let mut strategy = ready_to_trade_strategy();
        set_blind_recovery(&mut strategy, BlindRecoveryReason::CacheProbeFailed);

        let decision = strategy.entry_gate_decision_at(2_000);

        assert!(
            decision
                .blocked_by
                .contains(&EntryBlockReason::OnePositionInvariant(
                    ExposureOccupancy::BlindRecovery
                ))
        );
        assert_eq!(
            strategy.last_reported_exposure_occupancy.get(),
            Some(ExposureOccupancy::BlindRecovery)
        );
    }

    #[test]
    fn task5_entry_order_plan_uses_fok_and_side_specific_best_price() {
        let up = build_entry_order_plan(&EntryOrderPlanInputs {
            client_order_id: ClientOrderId::from("ENTRY-UP"),
            instrument_id: InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET"),
            outcome_side: OutcomeSide::Up,
            quantity: Quantity::non_zero(5.0, 0),
            price_precision: 2,
            best_bid: 0.43,
            best_ask: 0.45,
        })
        .expect("up entry should have a valid plan");
        let down = build_entry_order_plan(&EntryOrderPlanInputs {
            client_order_id: ClientOrderId::from("ENTRY-DOWN"),
            instrument_id: InstrumentId::from("condition-MKT-1-MKT-1-DOWN.POLYMARKET"),
            outcome_side: OutcomeSide::Down,
            quantity: Quantity::non_zero(5.0, 0),
            price_precision: 2,
            best_bid: 0.43,
            best_ask: 0.45,
        })
        .expect("down entry should have a valid plan");

        assert_eq!(up.order_side, OrderSide::Buy);
        assert_eq!(up.price, Price::new(0.45, 2));
        assert_eq!(up.time_in_force, TimeInForce::Fok);
        assert_eq!(down.order_side, OrderSide::Buy);
        assert_eq!(down.price, Price::new(0.45, 2));
        assert_eq!(down.time_in_force, TimeInForce::Fok);
    }

    #[test]
    fn task5_exit_decision_uses_hysteresis_boundary_and_fails_closed() {
        assert_eq!(
            evaluate_exit_decision(Some(12.0), Some(11.0), 1.0),
            ExitDecision::Exit
        );
        assert_eq!(
            evaluate_exit_decision(Some(12.0), Some(10.5), 1.0),
            ExitDecision::Hold
        );
        assert_eq!(
            evaluate_exit_decision(None, Some(10.0), 1.0),
            ExitDecision::ExitFailClosed
        );
        assert_eq!(
            evaluate_exit_decision(Some(12.0), Some(f64::NAN), 1.0),
            ExitDecision::ExitFailClosed
        );
    }

    #[test]
    fn expected_exit_submission_blocks_do_not_warn() {
        assert!(!should_warn_on_exit_submission_block(Some(
            "no_open_position"
        )));
        assert!(!should_warn_on_exit_submission_block(Some(
            "exit_already_pending"
        )));
        assert!(!should_warn_on_exit_submission_block(Some("exit_hold")));
        assert!(should_warn_on_exit_submission_block(Some(
            "exit_price_missing"
        )));
    }

    #[test]
    fn task5_forced_flat_predicates_cover_current_strategy_visible_triggers() {
        let reasons = evaluate_forced_flat_predicates(&ForcedFlatInputs {
            phase: SelectionPhase::Freeze,
            metadata_matches_selection: false,
            last_chainlink_ts_ms: Some(1_000),
            now_ms: 3_000,
            stale_chainlink_after_ms: 1_500,
            liquidity_available: Some(50.0),
            min_liquidity_required: 100.0,
            fast_venue_incoherent: true,
        });

        assert_eq!(
            reasons,
            vec![
                ForcedFlatReason::Freeze,
                ForcedFlatReason::StaleChainlink,
                ForcedFlatReason::ThinBook,
                ForcedFlatReason::MetadataMismatch,
                ForcedFlatReason::FastVenueIncoherent,
            ]
        );
    }

    #[test]
    fn task5_entry_gate_blocks_on_active_phase_forced_flat_reasons() {
        let mut strategy = ready_to_trade_strategy();
        strategy.active.last_reference_ts_ms = Some(1_000);
        strategy.active.books.up.liquidity_available = Some(50.0);
        strategy.active.books.down.liquidity_available = Some(50.0);
        strategy.active.fast_venue_incoherent = true;

        let decision = strategy.entry_gate_decision_at(3_000);

        assert_eq!(
            decision.blocked_by,
            vec![
                EntryBlockReason::ForcedFlat(ForcedFlatReason::StaleChainlink),
                EntryBlockReason::ForcedFlat(ForcedFlatReason::ThinBook),
                EntryBlockReason::ForcedFlat(ForcedFlatReason::FastVenueIncoherent),
            ]
        );
    }

    #[test]
    fn task5_cooldown_is_per_market_and_recovery_blocks_new_entries() {
        let mut strategy = ready_to_trade_strategy();
        strategy.arm_market_cooldown("MKT-1", 1_000);

        assert!(strategy.market_in_cooldown("MKT-1", 30_999));
        assert!(!strategy.market_in_cooldown("MKT-2", 30_999));

        set_blind_recovery(&mut strategy, BlindRecoveryReason::CacheProbeFailed);
        let decision = strategy.entry_gate_decision_at(2_000);

        assert!(
            decision
                .blocked_by
                .contains(&EntryBlockReason::RecoveryMode)
        );
    }

    #[test]
    fn inactive_expired_market_lifecycle_is_pruned_after_selection_update() {
        let mut strategy = ready_to_trade_strategy();
        strategy.record_market_fill("STALE", 0);

        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 31_001));

        assert!(!strategy.market_lifecycle.contains_key("STALE"));
        assert_eq!(strategy.market_churn_count("STALE"), 0);
    }

    #[test]
    fn tracked_market_lifecycle_is_retained_after_cooldown_expiry() {
        let mut strategy = ready_to_trade_strategy();
        let tracked_instrument = strategy.active.books.up.instrument_id.unwrap();
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: tracked_instrument,
            position_id: PositionId::from("P-LIFECYCLE-001"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );
        strategy.record_market_fill("MKT-1", 0);

        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 31_001));

        assert!(strategy.market_lifecycle.contains_key("MKT-1"));
        assert_eq!(strategy.market_churn_count("MKT-1"), 1);
    }

    #[test]
    fn task6_entry_evaluation_blocks_when_realized_vol_is_not_ready() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_101.0, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = None;
        strategy.pricing.realized_vol.last_ready_ts_ms = None;

        let decision = strategy.entry_evaluation_at(1_200);

        assert!(decision.gate.blocked_by.is_empty());
        assert_eq!(
            decision.pricing_blocked_by,
            vec![EntryPricingBlockReason::RealizedVolNotReady]
        );
        assert_eq!(decision.selected_side, None);
    }

    #[test]
    fn task6_entry_evaluation_computes_both_side_evs_from_live_state() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_100.4, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);

        let decision = strategy.entry_evaluation_at(1_200);

        assert!(decision.gate.blocked_by.is_empty());
        assert!(decision.pricing_blocked_by.is_empty());
        assert!(
            decision
                .fair_probability_up
                .is_some_and(|value| value > 0.5),
            "live pricing should infer an up edge from spot above strike"
        );
        assert!(decision.up_worst_case_ev_bps.is_some());
        assert!(decision.down_worst_case_ev_bps.is_some());
        assert!(
            decision
                .expected_ev_per_usdc
                .is_some_and(|value| value > 0.0)
        );
        assert!(
            decision
                .book_impact_cap_usdc
                .is_some_and(|value| value > 0.0)
        );
        assert!(
            decision
                .sized_notional_usdc
                .is_some_and(|value| value > 0.0)
        );
        assert_eq!(decision.selected_side, Some(OutcomeSide::Up));
    }

    #[test]
    fn task6_entry_evaluation_uses_live_uncertainty_band_probability() {
        let mut strategy =
            ready_to_trade_strategy_with_live_fees(Decimal::new(250, 2), Decimal::new(250, 2));
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_100.4, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
        strategy.pricing.last_lead_gap_probability = Some(0.02);
        strategy.pricing.last_jitter_penalty_probability = Some(0.01);

        let decision = strategy.entry_evaluation_at(1_200);

        assert!(decision.pricing_blocked_by.is_empty());
        assert!(
            decision
                .uncertainty_band_probability
                .is_some_and(|value| value > 0.0)
        );
    }

    #[test]
    fn task6_entry_evaluation_applies_theta_scaled_threshold_at_boundary() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_120.0, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
        strategy.pricing.realized_vol.bridge_valid_ms = 1_000_000;
        strategy.config.worst_case_ev_min_bps = 2_000;

        let baseline = strategy.entry_evaluation_at(1_200);
        assert_eq!(baseline.selected_side, Some(OutcomeSide::Up));

        strategy.config.theta_decay_factor = 100.0;
        strategy.active.last_reference_ts_ms = Some(291_000);
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_120.0, 291_000));
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(291_000);
        let near_expiry = strategy.entry_evaluation_at(291_000);

        assert!(near_expiry.gate.blocked_by.is_empty());
        assert!(near_expiry.pricing_blocked_by.is_empty());
        assert!(near_expiry.up_worst_case_ev_bps.is_some());
        assert!(near_expiry.min_worst_case_ev_bps.is_some());
        assert_eq!(near_expiry.selected_side, None);
        assert!(
            near_expiry
                .min_worst_case_ev_bps
                .zip(near_expiry.up_worst_case_ev_bps)
                .is_some_and(|(threshold, up_ev)| threshold >= up_ev),
            "theta-scaled threshold should close the entry boundary near expiry"
        );
    }

    #[test]
    fn entry_evaluation_log_fields_capture_parameters_and_omissions() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms: 1_200,
            topic: "platform.reference.test.chainlink".to_string(),
            fair_value: Some(3_100.5),
            confidence: 1.0,
            venues: vec![
                oracle_venue("chainlink", 1.0, 3_100.5, 1_200),
                orderbook_venue("bybit", 0.9, 3_101.0, 1_200),
            ],
        });
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);

        let evaluation = strategy.entry_evaluation_at(1_200);
        let submission = strategy.entry_submission_decision_at(1_200);
        let fields = strategy.entry_evaluation_log_fields_at(1_200, &submission);

        assert_eq!(fields.market_id.as_deref(), Some("MKT-1"));
        assert_eq!(fields.phase, SelectionPhase::Active);
        assert_eq!(fields.spot_venue_name.as_deref(), Some("bybit"));
        assert_eq!(fields.spot_price, Some(3_101.0));
        assert_eq!(fields.reference_fair_value, Some(3_100.5));
        assert_eq!(fields.interval_open, Some(3_100.0));
        assert_eq!(fields.realized_vol, Some(2.5));
        assert_eq!(fields.realized_vol_source_venue.as_deref(), Some("bybit"));
        assert_eq!(fields.realized_vol_source_ts_ms, Some(1_200));
        assert_eq!(fields.fair_probability_up, evaluation.fair_probability_up);
        assert_eq!(fields.selected_side, evaluation.selected_side);
        assert!(fields.uncertainty_band_probability.is_some());
        assert!(fields.uncertainty_band_live);
        assert_eq!(
            fields.uncertainty_band_reason,
            "derived_from_lead_gap_jitter_time_and_fee"
        );
        assert!(fields.lead_quality_policy_applied);
        assert!(fields.expected_ev_per_usdc.is_some_and(|value| value > 0.0));
        assert_eq!(fields.max_position_usdc, strategy.config.max_position_usdc);
        assert_eq!(fields.risk_lambda, strategy.config.risk_lambda);
        assert_eq!(
            fields.book_impact_cap_bps,
            strategy.config.book_impact_cap_bps
        );
        assert!(fields.book_impact_cap_usdc.is_some_and(|value| value > 0.0));
        assert!(fields.sized_notional_usdc.is_some_and(|value| value > 0.0));
        assert!(!fields.maker_rebate_available);
        assert!(!fields.category_available);
        assert!(!fields.final_fee_amount_known);
    }

    #[test]
    fn exit_evaluation_log_fields_use_position_context_after_rotation() {
        let fee_provider = RecordingFeeProvider::cold();
        fee_provider.set_fee("MKT-1-UP", Decimal::new(100, 2));
        fee_provider.set_fee("MKT-1-DOWN", Decimal::new(200, 2));
        fee_provider.set_fee("MKT-2-UP", Decimal::new(300, 2));
        fee_provider.set_fee("MKT-2-DOWN", Decimal::new(400, 2));

        let mut strategy = test_strategy_with_fee_provider(fee_provider);
        strategy.config.warmup_tick_count = 2;
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));
        strategy.active.interval_open = Some(3_100.0);
        strategy.active.warmup_count = 2;
        strategy.active.last_reference_ts_ms = Some(2_000);
        strategy.refresh_fee_readiness();
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: strategy.active.books.up.instrument_id.unwrap(),
            position_id: PositionId::from("P-UP-LOG-001"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(1.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );

        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
        strategy.active.interval_open = Some(3_200.0);
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_101.0, 2_000));
        strategy.pricing.realized_vol_source_venue = Some("bybit".to_string());
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(2_000);

        let decision = strategy.exit_submission_decision_at(2_000);
        let fields = strategy.exit_evaluation_log_fields_at(2_000, &decision);

        assert_eq!(fields.market_id.as_deref(), Some("MKT-1"));
        assert_eq!(fields.interval_open, Some(3_100.0));
        assert_eq!(fields.seconds_to_expiry, Some(299));
        assert_eq!(fields.realized_vol_source_venue.as_deref(), Some("bybit"));
        assert_eq!(fields.realized_vol_source_ts_ms, Some(2_000));
        assert_eq!(fields.up_fee_bps, Some(1.0));
        assert_eq!(fields.down_fee_bps, Some(2.0));
    }

    #[test]
    fn historical_entry_fee_rate_exit_ev_uses_entry_fee_from_submission_time() {
        let (mut strategy, fee_provider) =
            ready_to_trade_strategy_with_recording_fees(Decimal::new(100, 2), Decimal::ZERO);
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let client_order_id = ClientOrderId::from("ENTRY-HIST-FEE-001");
        let pending = pending_entry_state(
            &strategy,
            client_order_id,
            instrument_id,
            OutcomeSide::Up,
            strategy.active.books.up.clone(),
        );
        set_pending_entry(&mut strategy, pending);

        fee_provider.set_fee("MKT-1-UP", Decimal::new(300, 2));
        strategy
            .on_order_filled(&order_filled_event(
                client_order_id,
                instrument_id,
                PositionId::from("P-HIST-FEE-001"),
            ))
            .expect("entry fill should materialize position for exit EV test");

        let exit_ev_bps = strategy
            .current_exit_ev_bps_at(OutcomeSide::Up)
            .expect("historical entry fee test should produce exit EV");
        let total_entry_cost = 0.450 * (1.0 + 1.0 / BPS_DENOMINATOR);
        let net_exit_value = 0.500 * (1.0 - 3.0 / BPS_DENOMINATOR);
        let expected_exit_ev_bps =
            ((net_exit_value - total_entry_cost) / total_entry_cost) * BPS_DENOMINATOR;

        assert!((exit_ev_bps - expected_exit_ev_bps).abs() < 1e-9);
    }

    #[test]
    fn historical_entry_fee_rate_logs_known_for_strategy_managed_positions() {
        let (mut strategy, fee_provider) =
            ready_to_trade_strategy_with_recording_fees(Decimal::new(100, 2), Decimal::ZERO);
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let client_order_id = ClientOrderId::from("ENTRY-HIST-LOG-001");
        let pending = pending_entry_state(
            &strategy,
            client_order_id,
            instrument_id,
            OutcomeSide::Up,
            strategy.active.books.up.clone(),
        );
        set_pending_entry(&mut strategy, pending);

        fee_provider.set_fee("MKT-1-UP", Decimal::new(300, 2));
        strategy
            .on_order_filled(&order_filled_event(
                client_order_id,
                instrument_id,
                PositionId::from("P-HIST-LOG-001"),
            ))
            .expect("entry fill should materialize position for log test");

        let decision = strategy.exit_submission_decision_at(1_200);
        let fields = strategy.exit_evaluation_log_fields_at(1_200, &decision);

        assert!(fields.historical_entry_fee_rate_known);
        assert_eq!(
            fields.historical_entry_fee_rate_reason,
            "captured_from_strategy_entry_state"
        );
    }

    #[test]
    fn unknown_recovered_position_side_exits_fail_closed_using_tracked_book() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let instrument_id = InstrumentId::from("0xcondition-222.POLYMARKET");
        let mut tracked_book = OutcomeBookState::from_instrument_id(instrument_id);
        tracked_book.last_observed_instrument_id = Some(instrument_id);
        tracked_book.best_bid = Some(0.520);
        tracked_book.best_ask = Some(0.530);
        tracked_book.liquidity_available = Some(100.0);
        set_managed_position(
            &mut strategy,
            OpenPositionState {
                market_id: None,
                instrument_id,
                position_id: PositionId::from("P-UNKNOWN-001"),
                outcome_side: None,
                outcome_fees: OutcomeFeeState::default(),
                historical_entry_fee_bps: None,
                entry_order_side: OrderSide::Buy,
                side: PositionSide::Long,
                quantity: Quantity::new(5.0, 2),
                avg_px_open: 0.480,
                interval_open: None,
                selection_published_at_ms: None,
                seconds_to_expiry_at_selection: None,
                book: tracked_book,
            },
            ManagedPositionOrigin::RecoveryBootstrap,
        );

        let decision = strategy.exit_submission_decision_at(2_000);

        assert_eq!(
            decision.evaluation.exit_decision,
            Some(ExitDecision::ExitFailClosed)
        );
        assert_eq!(decision.instrument_id, Some(instrument_id));
        assert_eq!(decision.order_side, Some(OrderSide::Sell));
        assert_eq!(decision.price, Some(0.520));
        assert_eq!(decision.quantity, Some(Quantity::new(5.0, 2)));
        assert_eq!(decision.blocked_reason, None);
    }

    #[test]
    fn quarantined_legacy_short_position_blocks_exit_submission() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let instrument_id = InstrumentId::from("0xcondition-legacy-222.POLYMARKET");
        let mut tracked_book = OutcomeBookState::from_instrument_id(instrument_id);
        tracked_book.last_observed_instrument_id = Some(instrument_id);
        tracked_book.best_bid = Some(0.520);
        tracked_book.best_ask = Some(0.530);
        tracked_book.liquidity_available = Some(100.0);
        set_unsupported_observed(
            &mut strategy,
            OpenPositionState {
                market_id: None,
                instrument_id,
                position_id: PositionId::from("P-LEGACY-SHORT-001"),
                outcome_side: None,
                outcome_fees: OutcomeFeeState::default(),
                historical_entry_fee_bps: None,
                entry_order_side: OrderSide::Sell,
                side: PositionSide::Short,
                quantity: Quantity::new(5.0, 2),
                avg_px_open: 0.480,
                interval_open: None,
                selection_published_at_ms: None,
                seconds_to_expiry_at_selection: None,
                book: tracked_book,
            },
            UnsupportedObservedReason::BootstrappedUnsupportedContract,
        );

        let decision = strategy.exit_submission_decision_at(2_000);

        assert_eq!(decision.evaluation.exit_decision, None);
        assert_eq!(decision.instrument_id, None);
        assert_eq!(decision.order_side, None);
        assert_eq!(decision.price, None);
        assert_eq!(decision.quantity, None);
        assert_eq!(decision.blocked_reason, Some("exit_decision_unavailable"));
    }

    #[test]
    fn task6_exit_submission_decision_forced_flat_submits_for_open_up_position() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.active.phase = SelectionPhase::Freeze;
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: strategy.active.books.up.instrument_id.unwrap(),
            position_id: PositionId::from("P-UP-001"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );

        let decision = strategy.exit_submission_decision_at(1_200);

        assert_eq!(decision.order_side, Some(OrderSide::Sell));
        assert_eq!(
            decision.instrument_id,
            strategy.active.books.up.instrument_id
        );
        assert_eq!(decision.price, strategy.active.books.up.best_bid);
        assert_eq!(decision.quantity, Some(Quantity::new(10.0, 2)));
        assert_eq!(decision.forced_flat_reasons, vec![ForcedFlatReason::Freeze]);
    }

    #[test]
    fn task6_exit_submission_decision_forced_flat_submits_for_open_down_position() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.active.phase = SelectionPhase::Freeze;
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: strategy.active.books.down.instrument_id.unwrap(),
            position_id: PositionId::from("P-DOWN-001"),
            outcome_side: Some(OutcomeSide::Down),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(12.0, 2),
            avg_px_open: 0.480,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.down.clone(),
        };
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );

        let decision = strategy.exit_submission_decision_at(1_200);

        assert_eq!(decision.order_side, Some(OrderSide::Sell));
        assert_eq!(
            decision.instrument_id,
            strategy.active.books.down.instrument_id
        );
        assert_eq!(decision.price, strategy.active.books.down.best_bid);
        assert_eq!(decision.quantity, Some(Quantity::new(12.0, 2)));
        assert_eq!(decision.forced_flat_reasons, vec![ForcedFlatReason::Freeze]);
    }

    #[test]
    fn task6_exit_submission_decision_uses_live_hold_vs_exit_boundary() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: strategy.active.books.up.instrument_id.unwrap(),
            position_id: PositionId::from("P-UP-002"),
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            entry_order_side: OrderSide::Buy,
            side: PositionSide::Long,
            quantity: Quantity::new(10.0, 2),
            avg_px_open: 0.450,
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_099.5, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);

        let decision = strategy.exit_submission_decision_at(1_200);

        assert!(decision.forced_flat_reasons.is_empty());
        assert_eq!(decision.order_side, Some(OrderSide::Sell));
        assert_eq!(
            decision.instrument_id,
            strategy.active.books.up.instrument_id
        );
        assert_eq!(decision.price, strategy.active.books.up.best_bid);
        assert_eq!(decision.quantity, Some(Quantity::new(10.0, 2)));
        assert_eq!(decision.blocked_reason, None);
    }

    #[test]
    fn exposure_entry_reconcile_pending_preserves_context_and_blocks_new_entries() {
        let strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let pending = PendingEntryState {
            client_order_id: ClientOrderId::from("ENTRY-RECONCILE-001"),
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            outcome_side: Some(OutcomeSide::Up),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: Some(0.0),
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: strategy.active.books.up.clone(),
        };
        let exposure = ExposureState::EntryReconcilePending {
            pending: pending.clone(),
            reason: EntryReconcileReason::AwaitingPositionMaterialization,
        };

        assert_eq!(exposure.pending_entry(), Some(&pending));
        assert_eq!(
            exposure.occupancy(),
            Some(ExposureOccupancy::EntryReconcilePending)
        );
        assert!(exposure.blocks_new_entries());
    }

    #[test]
    fn exposure_exit_pending_requires_both_fill_and_close_to_become_flat() {
        let strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let managed = ManagedPositionState {
            position: OpenPositionState {
                market_id: Some("MKT-1".to_string()),
                instrument_id,
                position_id: PositionId::from("P-EXIT-STATE-001"),
                outcome_side: Some(OutcomeSide::Up),
                outcome_fees: strategy.active.outcome_fees.clone(),
                historical_entry_fee_bps: Some(0.0),
                entry_order_side: OrderSide::Buy,
                side: PositionSide::Long,
                quantity: Quantity::new(10.0, 2),
                avg_px_open: 0.450,
                interval_open: Some(3_100.0),
                selection_published_at_ms: Some(1_000),
                seconds_to_expiry_at_selection: Some(300),
                book: strategy.active.books.up.clone(),
            },
            origin: ManagedPositionOrigin::StrategyEntry,
        };
        let mut exit_pending = ExitPendingState {
            position: Some(managed.clone()),
            pending_exit: PendingExitState {
                client_order_id: ClientOrderId::from("EXIT-STATE-001"),
                market_id: Some("MKT-1".to_string()),
                position_id: Some(PositionId::from("P-EXIT-STATE-001")),
                fill_received: false,
                close_received: false,
            },
        };

        assert!(!exit_pending.is_terminal());
        exit_pending.pending_exit.fill_received = true;
        assert!(!exit_pending.is_terminal());
        exit_pending.pending_exit.close_received = true;
        assert!(exit_pending.is_terminal());
        assert_eq!(
            exit_pending
                .position
                .as_ref()
                .map(|state| state.position.position_id),
            Some(PositionId::from("P-EXIT-STATE-001"))
        );
    }

    #[test]
    fn exposure_managed_recovery_origin_is_explicit_without_recovery_boolean() {
        let strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let managed = ExposureState::Managed(ManagedPositionState {
            position: OpenPositionState {
                market_id: Some("MKT-1".to_string()),
                instrument_id,
                position_id: PositionId::from("P-RECOVERY-001"),
                outcome_side: Some(OutcomeSide::Up),
                outcome_fees: strategy.active.outcome_fees.clone(),
                historical_entry_fee_bps: None,
                entry_order_side: OrderSide::Buy,
                side: PositionSide::Long,
                quantity: Quantity::new(5.0, 2),
                avg_px_open: 0.440,
                interval_open: Some(3_100.0),
                selection_published_at_ms: Some(1_000),
                seconds_to_expiry_at_selection: Some(300),
                book: strategy.active.books.up.clone(),
            },
            origin: ManagedPositionOrigin::RecoveryBootstrap,
        });

        let managed = managed
            .managed_position()
            .expect("managed exposure should return managed position");
        assert_eq!(managed.origin, ManagedPositionOrigin::RecoveryBootstrap);
        assert_eq!(
            managed.position.position_id,
            PositionId::from("P-RECOVERY-001")
        );
    }
}
