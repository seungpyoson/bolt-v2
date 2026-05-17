use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, VecDeque},
    rc::Rc,
    str::FromStr,
};

use anyhow::{Context, Result};
use nautilus_common::{actor::DataActor, component::Component, timer::TimeEvent};
#[cfg(not(test))]
use nautilus_model::enums::BookType;
use nautilus_model::{data::QuoteTick, enums::PositionSide};
use nautilus_model::{
    enums::{BookAction, OmsType as NtOmsType, OrderSide, TimeInForce},
    identifiers::{ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId},
    instruments::{Instrument, InstrumentAny},
    types::{Price, Quantity},
};
use nautilus_system::trader::Trader;
use nautilus_trading::{Strategy, StrategyConfig, StrategyCore, nautilus_strategy};
use rust_decimal::{Decimal, prelude::ToPrimitive};
use serde::Deserialize;
use toml::Value;

use crate::{
    bolt_v3_decision_evidence::{BoltV3OrderIntentEvidence, BoltV3OrderIntentKind},
    bolt_v3_market_families::{self, MarketSelectionTarget},
    bolt_v3_submit_admission::BoltV3SubmitAdmissionRequest,
    strategies::registry::{
        BoxedStrategy, FeeProvider, StrategyBuildContext, StrategyBuilder, ValidationError,
    },
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

macro_rules! binary_oracle_edge_taker_config_fields {
    ($macro:ident) => {
        $macro! {
            strategy_id: String => String;
            order_id_tag: String => String;
            oms_type: String => String;
            client_id: String => String;
            configured_target_id: String => String;
            target_kind: String => String;
            rotating_market_family: String => String;
            underlying_asset: String => String;
            cadence_seconds: u64 => Integer;
            cadence_slug_token: String => String;
            market_selection_rule: String => String;
            retry_interval_seconds: u64 => Integer;
            blocked_after_seconds: u64 => Integer;
            reference_venue: String => String;
            reference_instrument_id: String => String;
            use_uuid_client_order_ids: bool => Boolean;
            use_hyphens_in_client_order_ids: bool => Boolean;
            external_order_claims: Vec<String> => Array;
            manage_contingent_orders: bool => Boolean;
            manage_gtd_expiry: bool => Boolean;
            manage_stop: bool => Boolean;
            market_exit_interval_ms: u64 => Integer;
            market_exit_max_attempts: u64 => Integer;
            market_exit_time_in_force: String => String;
            market_exit_reduce_only: bool => Boolean;
            log_events: bool => Boolean;
            log_commands: bool => Boolean;
            log_rejected_due_post_only_as_warning: bool => Boolean;
            warmup_tick_count: u64 => Integer;
            reentry_cooldown_secs: u64 => Integer;
            order_notional_target: f64 => Float;
            maximum_position_notional: f64 => Float;
            book_impact_cap_bps: u64 => Integer;
            risk_lambda: f64 => Float;
            edge_threshold_basis_points: i64 => Integer;
            exit_hysteresis_bps: i64 => Integer;
            vol_window_secs: u64 => Integer;
            vol_gap_reset_secs: u64 => Integer;
            vol_min_observations: u64 => Integer;
            vol_bridge_valid_secs: u64 => Integer;
            pricing_kurtosis: f64 => Float;
            theta_decay_factor: f64 => Float;
            forced_flat_stale_reference_ms: u64 => Integer;
            forced_flat_thin_book_min_liquidity: f64 => Float;
            lead_agreement_min_corr: f64 => Float;
            lead_jitter_max_ms: u64 => Integer;
        }
    };
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct BinaryOracleEdgeTakerOrderConfig {
    side: String,
    position_side: String,
    order_type: String,
    time_in_force: String,
    is_post_only: bool,
    is_reduce_only: bool,
    is_quote_quantity: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BinaryOracleEdgeTakerFieldType {
    String,
    Boolean,
    Integer,
    Float,
    Array,
    Table,
}

impl BinaryOracleEdgeTakerFieldType {
    fn expected(self) -> &'static str {
        match self {
            Self::String => stringify!(string),
            Self::Boolean => stringify!(boolean),
            Self::Integer => stringify!(integer),
            Self::Float => stringify!(float),
            Self::Array => stringify!(array),
            Self::Table => stringify!(table),
        }
    }

    fn article(self) -> &'static str {
        match self {
            Self::String | Self::Boolean | Self::Float | Self::Table => stringify!(a),
            Self::Integer | Self::Array => stringify!(an),
        }
    }

    fn matches(self, value: &Value) -> bool {
        match self {
            Self::String => value.as_str().is_some(),
            Self::Boolean => value.as_bool().is_some(),
            Self::Integer => value.as_integer().is_some(),
            Self::Float => value.as_float_or_integer().is_some(),
            Self::Array => value.as_array().is_some(),
            Self::Table => value.as_table().is_some(),
        }
    }
}

macro_rules! define_config_struct {
    ($( $field:ident : $ty:ty => $field_type:ident; )+) => {
        #[derive(Debug, Clone, PartialEq, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct BinaryOracleEdgeTakerConfig {
            $( $field: $ty, )+
            entry_order: BinaryOracleEdgeTakerOrderConfig,
            exit_order: BinaryOracleEdgeTakerOrderConfig,
        }
    };
}

macro_rules! match_config_field_names {
    ($( $field:ident : $ty:ty => $field_type:ident; )+) => {
        $( stringify!($field) )|+
    };
}

macro_rules! validate_config_fields_impl {
    ($( $field:ident : $ty:ty => $field_type:ident; )+) => {
        |table: &toml::map::Map<String, Value>, field_prefix: &str, errors: &mut Vec<ValidationError>| {
            $(
                let field = format!("{field_prefix}.{}", stringify!($field));
                let field_type = BinaryOracleEdgeTakerFieldType::$field_type;
                match table.get(stringify!($field)) {
                    None => BinaryOracleEdgeTakerBuilder::push_missing(
                        errors,
                        field,
                        concat!(stringify!(missing_), stringify!($field)),
                        field_type,
                    ),
                    Some(value) if !field_type.matches(value) => {
                        BinaryOracleEdgeTakerBuilder::push_wrong_type(
                            errors,
                            field,
                            field_type,
                            value,
                        );
                    }
                    Some(_) => {}
                }
            )+
        }
    };
}

macro_rules! binary_oracle_edge_taker_order_fields {
    ($macro:ident) => {
        $macro! {
            side => String;
            position_side => String;
            order_type => String;
            time_in_force => String;
            is_post_only => Boolean;
            is_reduce_only => Boolean;
            is_quote_quantity => Boolean;
        }
    };
}

macro_rules! match_order_field_names {
    ($( $field:ident => $field_type:ident; )+) => {
        $( stringify!($field) )|+
    };
}

macro_rules! validate_order_fields_impl {
    ($( $field:ident => $field_type:ident; )+) => {
        |table: &toml::map::Map<String, Value>,
         field_prefix: &str,
         errors: &mut Vec<ValidationError>| {
            $(
                BinaryOracleEdgeTakerBuilder::validate_order_field(
                    table,
                    field_prefix,
                    stringify!($field),
                    concat!(stringify!(missing_), stringify!($field)),
                    BinaryOracleEdgeTakerFieldType::$field_type,
                    errors,
                );
            )+
        }
    };
}

binary_oracle_edge_taker_config_fields!(define_config_struct);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelectionPhase {
    Active,
    Freeze,
    Idle,
}

#[derive(Debug, Clone, PartialEq)]
struct CandidateOutcome {
    instrument_id: String,
}

#[derive(Debug, Clone, PartialEq)]
struct CandidateMarket {
    market_id: String,
    instrument_id: String,
    up: CandidateOutcome,
    down: CandidateOutcome,
    price_to_beat: Option<f64>,
    start_ts_ms: u64,
    seconds_to_end: u64,
}

#[derive(Debug, Clone, PartialEq)]
enum SelectionState {
    Active {
        market: CandidateMarket,
    },
    #[cfg(test)]
    Freeze {
        market: CandidateMarket,
        reason: String,
    },
    Idle {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct SelectionDecision {
    ruleset_id: String,
    state: SelectionState,
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeSelectionSnapshot {
    ruleset_id: String,
    decision: SelectionDecision,
    eligible_candidates: Vec<CandidateMarket>,
    published_at_ms: u64,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
enum VenueHealth {
    Healthy,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VenueKind {
    Orderbook,
    Oracle,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
struct EffectiveVenueState {
    venue_name: String,
    base_weight: f64,
    effective_weight: f64,
    stale: bool,
    health: VenueHealth,
    observed_ts_ms: Option<u64>,
    venue_kind: VenueKind,
    observed_price: Option<f64>,
    observed_bid: Option<f64>,
    observed_ask: Option<f64>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
struct ReferenceSnapshot {
    ts_ms: u64,
    topic: String,
    fair_value: Option<f64>,
    confidence: f64,
    venues: Vec<EffectiveVenueState>,
}

#[derive(Debug, Clone, PartialEq)]
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
    fn empty() -> Self {
        Self {
            instrument_id: None,
            last_observed_instrument_id: None,
            bid_levels: BTreeMap::new(),
            ask_levels: BTreeMap::new(),
            best_bid: None,
            best_ask: None,
            liquidity_available: None,
        }
    }

    fn from_instrument_id(instrument_id: InstrumentId) -> Self {
        Self {
            instrument_id: Some(instrument_id),
            last_observed_instrument_id: None,
            bid_levels: BTreeMap::new(),
            ask_levels: BTreeMap::new(),
            best_bid: None,
            best_ask: None,
            liquidity_available: None,
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
                        if is_positive_finite(size) {
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

    fn executable_price_for_order_side(&self, order_side: OrderSide) -> Option<f64> {
        match order_side {
            OrderSide::Buy => self.best_ask,
            OrderSide::Sell => self.best_bid,
            _ => None,
        }
        .filter(|value| is_positive_finite(*value))
    }

    fn max_execution_within_vwap_slippage_bps(
        &self,
        order_side: OrderSide,
        slippage_bps: u64,
    ) -> Option<ImpactCappedExecution> {
        let slippage = slippage_bps as f64 / BPS_DENOMINATOR;
        match order_side {
            OrderSide::Buy => {
                let best_ask = self.executable_price_for_order_side(OrderSide::Buy)?;
                let allowed_vwap = best_ask * (UNIT_F64 + slippage);
                max_execution_within_vwap_limit(
                    self.ask_levels
                        .iter()
                        .map(|(price, size)| (price.as_f64(), *size))
                        .collect(),
                    allowed_vwap,
                    true,
                )
            }
            OrderSide::Sell => {
                let best_bid = self.executable_price_for_order_side(OrderSide::Sell)?;
                let allowed_vwap = best_bid * (UNIT_F64 - slippage);
                max_execution_within_vwap_limit(
                    self.bid_levels
                        .iter()
                        .rev()
                        .map(|(price, size)| (price.as_f64(), *size))
                        .collect(),
                    allowed_vwap,
                    false,
                )
            }
            _ => None,
        }
    }
}

fn max_execution_within_vwap_limit(
    levels: Vec<(f64, f64)>,
    allowed_vwap: f64,
    is_buy: bool,
) -> Option<ImpactCappedExecution> {
    if !is_positive_finite(allowed_vwap) {
        return None;
    }

    let mut cumulative_qty = ZERO_F64;
    let mut cumulative_notional = ZERO_F64;

    for (price, size) in levels {
        if !is_positive_finite(price) || !is_positive_finite(size) {
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
            if denominator <= ZERO_F64 {
                size
            } else {
                ((allowed_vwap * cumulative_qty - cumulative_notional) / denominator)
                    .clamp(ZERO_F64, size)
            }
        } else {
            let denominator = allowed_vwap - price;
            if denominator <= ZERO_F64 {
                size
            } else {
                ((cumulative_notional - allowed_vwap * cumulative_qty) / denominator)
                    .clamp(ZERO_F64, size)
            }
        };

        let total_qty = cumulative_qty + partial_qty;
        let total_notional = cumulative_notional + partial_qty * price;
        return is_positive_finite(total_qty).then_some(ImpactCappedExecution {
            quantity: total_qty,
            vwap_price: total_notional / total_qty,
        });
    }

    is_positive_finite(cumulative_qty).then_some(ImpactCappedExecution {
        quantity: cumulative_qty,
        vwap_price: cumulative_notional / cumulative_qty,
    })
}

#[derive(Debug, Clone, PartialEq)]
struct OutcomePreparedBooks {
    up: OutcomeBookState,
    down: OutcomeBookState,
}

impl OutcomePreparedBooks {
    fn empty() -> Self {
        Self {
            up: OutcomeBookState::empty(),
            down: OutcomeBookState::empty(),
        }
    }

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

#[derive(Debug, Clone, PartialEq)]
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
}

#[derive(Debug, Clone, PartialEq)]
struct FastSpotObservation {
    venue_name: String,
    price: f64,
    observed_ts_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VenueTimingState {
    last_observed_ts_ms: Option<u64>,
    last_interval_ms: Option<u64>,
}

impl VenueTimingState {
    fn empty() -> Self {
        Self {
            last_observed_ts_ms: None,
            last_interval_ms: None,
        }
    }
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
    fn from_config(config: &BinaryOracleEdgeTakerConfig) -> Self {
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
        if !is_positive_finite(sample.price) {
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
        let min_observations = self.min_observations.max(MIN_OBSERVATION_COUNT) as usize;
        let mut observation_count = INITIAL_COUNTER_USIZE;
        let mut elapsed_ms = INITIAL_COUNTER_U64;
        let mut sum_squared_returns = ZERO_F64;

        let mut iter = self.samples.iter();
        let mut previous = iter.next()?;
        for current in iter {
            let dt_ms = current.ts_ms.saturating_sub(previous.ts_ms);
            if dt_ms == 0 {
                previous = current;
                continue;
            }
            if !is_positive_finite(current.price) || !is_positive_finite(previous.price) {
                return None;
            }

            let log_return = (current.price / previous.price).ln();
            if !log_return.is_finite() {
                return None;
            }

            sum_squared_returns += log_return.powi(POWER_OF_TWO);
            elapsed_ms = elapsed_ms.saturating_add(dt_ms);
            observation_count += COUNTER_INCREMENT;
            previous = current;
        }

        if observation_count < min_observations || elapsed_ms == 0 {
            return None;
        }

        let elapsed_secs = elapsed_ms as f64 / MILLIS_PER_SECOND_F64;
        let annualized_variance = (sum_squared_returns / elapsed_secs) * SECONDS_PER_YEAR_F64;
        let vol = annualized_variance.sqrt();
        if is_positive_finite(vol) {
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
        side: Option<PositionSide>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConfiguredPositionContract {
    entry_order_side: OrderSide,
    entry_position_side: PositionSide,
    exit_order_side: OrderSide,
    exit_position_side: PositionSide,
}

fn supports_strategy_managed_position(
    entry_order_side: OrderSide,
    side: PositionSide,
    contract: ConfiguredPositionContract,
) -> bool {
    supports_strategy_position_contract(contract)
        && entry_order_side == contract.entry_order_side
        && side == contract.entry_position_side
        && is_observed_open_side(side)
}

fn supports_strategy_position_contract(contract: ConfiguredPositionContract) -> bool {
    expected_position_side_for_entry_order(contract.entry_order_side)
        .is_some_and(|side| side == contract.entry_position_side)
        && expected_exit_order_side_for_position(contract.exit_position_side)
            .is_some_and(|side| side == contract.exit_order_side)
        && contract.entry_position_side == contract.exit_position_side
        && is_observed_open_side(contract.entry_position_side)
}

fn expected_position_side_for_entry_order(order_side: OrderSide) -> Option<PositionSide> {
    match order_side {
        OrderSide::Buy => Some(PositionSide::Long),
        OrderSide::Sell => Some(PositionSide::Short),
        _ => None,
    }
}

fn expected_exit_order_side_for_position(position_side: PositionSide) -> Option<OrderSide> {
    match position_side {
        PositionSide::Long => Some(OrderSide::Sell),
        PositionSide::Short => Some(OrderSide::Buy),
        _ => None,
    }
}

fn is_observed_open_side(side: PositionSide) -> bool {
    matches!(side, PositionSide::Long | PositionSide::Short)
}

fn order_price_for_side(book: &OutcomeBookState, order_side: OrderSide) -> Option<f64> {
    book.executable_price_for_order_side(order_side)
}

fn infer_strategy_position_side_from_entry_fill(
    entry_order_side: OrderSide,
    configured_entry_order_side: OrderSide,
    configured_position_side: PositionSide,
) -> Option<PositionSide> {
    (entry_order_side == configured_entry_order_side).then_some(configured_position_side)
}

fn managed_position_effective_entry_cost(
    position: &OpenPositionState,
    configured_entry_order_side: OrderSide,
    configured_position_side: PositionSide,
) -> Option<f64> {
    (position.entry_order_side == configured_entry_order_side
        && position.side == configured_position_side)
        .then_some(position.avg_px_open)
        .filter(|effective_cost| is_positive_finite(*effective_cost))
}

fn managed_position_exit_order(
    position: &OpenPositionState,
    configured_order_side: OrderSide,
    configured_position_side: PositionSide,
) -> Option<(OrderSide, f64)> {
    (position.side == configured_position_side)
        .then_some((
            configured_order_side,
            order_price_for_side(&position.book, configured_order_side)?,
        ))
        .filter(|(_, price)| is_positive_finite(*price))
}

fn managed_position_exit_value(
    position: &OpenPositionState,
    configured_order_side: OrderSide,
    configured_position_side: PositionSide,
) -> Option<f64> {
    let value = (position.side == configured_position_side)
        .then(|| order_price_for_side(&position.book, configured_order_side))
        .flatten()?;
    Some(value).filter(|value| is_positive_finite(*value))
}

impl PricingState {
    fn from_config(config: &BinaryOracleEdgeTakerConfig) -> Self {
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

    fn observe_reference_quote(
        &mut self,
        quote: &FastSpotObservation,
        min_agreement_corr: f64,
        max_jitter_ms: u64,
    ) {
        if !is_positive_finite(quote.price) {
            return;
        }

        self.last_reference_fair_value = Some(quote.price);
        self.lead_quality_policy_applied = true;

        let jitter_ms = self.record_reference_quote_timing(&quote.venue_name, quote.observed_ts_ms);
        let agreement_corr = price_agreement_corr(quote.price, quote.price)
            .expect("validated reference quote price should self-agree");
        let lead_gap_probability = price_gap_probability(quote.price, quote.price)
            .expect("validated reference quote price should yield a gap");
        let eligible = agreement_corr >= min_agreement_corr
            && jitter_ms <= max_jitter_ms
            && sanitize_probability(lead_gap_probability).is_some();

        if eligible {
            let selected_realized_vol = {
                let estimator_template = self.realized_vol.empty_like();
                let estimator = self
                    .realized_vol_by_venue
                    .entry(quote.venue_name.clone())
                    .or_insert_with(|| estimator_template.clone());
                let _ = estimator.observe(quote);
                estimator.clone()
            };
            self.realized_vol = selected_realized_vol;
            self.realized_vol_source_venue = Some(quote.venue_name.clone());
            self.fast_spot = Some(quote.clone());
            self.last_lead_gap_probability = Some(lead_gap_probability);
            self.last_jitter_penalty_probability = Some(if max_jitter_ms == 0 {
                ZERO_F64
            } else {
                clamp_probability(jitter_ms as f64 / max_jitter_ms as f64)
            });
            self.last_lead_agreement_corr = Some(agreement_corr);
            self.last_fast_venue_age_ms = Some(INITIAL_COUNTER_U64);
            self.last_fast_venue_jitter_ms = Some(jitter_ms);
            self.fast_venue_incoherent = false;
        } else {
            self.fast_spot = None;
            self.last_lead_gap_probability = Some(lead_gap_probability);
            self.last_jitter_penalty_probability = Some(if max_jitter_ms == 0 {
                ZERO_F64
            } else {
                clamp_probability(jitter_ms as f64 / max_jitter_ms as f64)
            });
            self.last_lead_agreement_corr = Some(agreement_corr);
            self.last_fast_venue_age_ms = Some(INITIAL_COUNTER_U64);
            self.last_fast_venue_jitter_ms = Some(jitter_ms);
            self.fast_venue_incoherent = true;
        }
    }

    fn record_reference_quote_timing(&mut self, venue_name: &str, observed_ts_ms: u64) -> u64 {
        let timing = self
            .venue_timing
            .entry(venue_name.to_string())
            .or_insert_with(VenueTimingState::empty);
        let current_interval_ms = timing
            .last_observed_ts_ms
            .map(|last_ts_ms| observed_ts_ms.saturating_sub(last_ts_ms));
        let jitter_ms = match (current_interval_ms, timing.last_interval_ms) {
            (Some(current_interval_ms), Some(last_interval_ms)) => {
                current_interval_ms.abs_diff(last_interval_ms)
            }
            _ => INITIAL_COUNTER_U64,
        };
        timing.last_observed_ts_ms = Some(observed_ts_ms);
        timing.last_interval_ms = current_interval_ms;
        jitter_ms
    }

    #[cfg(test)]
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

    #[cfg(test)]
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

    #[cfg(test)]
    fn selected_realized_vol_for_candidate(
        &self,
        candidate: &LeadVenueSignal,
    ) -> RealizedVolEstimator {
        self.realized_vol_by_venue
            .get(&candidate.venue_name)
            .cloned()
            .unwrap_or_else(|| {
                log::error!(
                    "binary_oracle_edge_taker selected lead venue missing realized-vol state: venue={}",
                    candidate.venue_name
                );
                self.realized_vol.empty_like()
            })
    }

    fn spot_price(&self) -> Option<f64> {
        self.fast_spot.as_ref().map(|spot| spot.price)
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

    #[cfg(test)]
    fn build_lead_venue_signals(&mut self, snapshot: &ReferenceSnapshot) -> Vec<LeadVenueSignal> {
        let agreement_anchor = best_healthy_oracle_price(snapshot).or(snapshot.fair_value);
        let reference_anchor = snapshot.fair_value;

        snapshot
            .venues
            .iter()
            .filter_map(|venue| {
                if venue.venue_kind != VenueKind::Orderbook
                    || venue.stale
                    || !matches!(venue.health, VenueHealth::Healthy)
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
                    .or_insert_with(VenueTimingState::empty);
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

                let agreement_anchor =
                    agreement_anchor.filter(|anchor| anchor.is_finite() && *anchor > 0.0)?;
                let reference_anchor =
                    reference_anchor.filter(|anchor| anchor.is_finite() && *anchor > 0.0)?;
                let agreement_corr = price_agreement_corr(observed_price, agreement_anchor)?;
                let lead_gap_probability = price_gap_probability(observed_price, reference_anchor)?;

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct OutcomeBookSubscriptions {
    up_instrument_id: Option<InstrumentId>,
    down_instrument_id: Option<InstrumentId>,
    tracked_position_instrument_id: Option<InstrumentId>,
}

impl OutcomeBookSubscriptions {
    fn empty() -> Self {
        Self {
            up_instrument_id: None,
            down_instrument_id: None,
            tracked_position_instrument_id: None,
        }
    }

    fn from_market(market: &CandidateMarket) -> Self {
        Self {
            up_instrument_id: Some(InstrumentId::from(market.up.instrument_id.as_str())),
            down_instrument_id: Some(InstrumentId::from(market.down.instrument_id.as_str())),
            tracked_position_instrument_id: None,
        }
    }

    fn is_same_market(&self, other: &Self) -> bool {
        self.up_instrument_id == other.up_instrument_id
            && self.down_instrument_id == other.down_instrument_id
            && self.tracked_position_instrument_id == other.tracked_position_instrument_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OutcomeFeeState {
    up_instrument_id: Option<InstrumentId>,
    down_instrument_id: Option<InstrumentId>,
    up_ready: bool,
    down_ready: bool,
}

impl OutcomeFeeState {
    fn empty() -> Self {
        Self {
            up_instrument_id: None,
            down_instrument_id: None,
            up_ready: false,
            down_ready: false,
        }
    }

    fn from_market(market: &CandidateMarket) -> Self {
        Self {
            up_instrument_id: Some(InstrumentId::from(market.up.instrument_id.as_str())),
            down_instrument_id: Some(InstrumentId::from(market.down.instrument_id.as_str())),
            up_ready: false,
            down_ready: false,
        }
    }

    fn instrument_ids(&self) -> Vec<InstrumentId> {
        [self.up_instrument_id, self.down_instrument_id]
            .into_iter()
            .flatten()
            .collect()
    }

    fn market_ready(&self) -> bool {
        self.up_ready && self.down_ready
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MarketLifecycleLedger {
    cooldown_expires_at_ms: Option<u64>,
    churn_count: u64,
}

impl MarketLifecycleLedger {
    fn empty() -> Self {
        Self {
            cooldown_expires_at_ms: None,
            churn_count: INITIAL_COUNTER_U64,
        }
    }

    fn in_cooldown(&self, now_ms: u64) -> bool {
        self.cooldown_expires_at_ms
            .is_some_and(|expiry_ms| now_ms < expiry_ms)
    }
}

impl ActiveMarketState {
    fn idle() -> Self {
        Self {
            phase: SelectionPhase::Idle,
            market_id: None,
            instrument_id: None,
            outcome_fees: OutcomeFeeState::empty(),
            price_to_beat: None,
            interval_start_ms: None,
            selection_published_at_ms: None,
            seconds_to_expiry_at_selection: None,
            interval_open: None,
            last_reference_ts_ms: None,
            warmup_count: INITIAL_COUNTER_U64,
            warmup_target: INITIAL_COUNTER_U64,
            books: OutcomePreparedBooks::empty(),
            fast_venue_incoherent: false,
            forced_flat: false,
        }
    }

    fn from_snapshot(snapshot: &RuntimeSelectionSnapshot, warmup_target: u64) -> Self {
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
            instrument_id: Some(InstrumentId::from(market.instrument_id.as_str())),
            outcome_fees: OutcomeFeeState::from_market(market),
            price_to_beat: market.price_to_beat,
            interval_start_ms: Some(market.start_ts_ms),
            selection_published_at_ms: None,
            seconds_to_expiry_at_selection: Some(market.seconds_to_end),
            interval_open: None,
            last_reference_ts_ms: None,
            warmup_count: INITIAL_COUNTER_U64,
            warmup_target,
            books: OutcomePreparedBooks::from_market(market),
            fast_venue_incoherent: false,
            forced_flat,
        }
    }

    fn same_boundary(&self, other: &Self) -> bool {
        self.phase == other.phase
            && self.market_id == other.market_id
            && self.instrument_id == other.instrument_id
            && self.interval_start_ms == other.interval_start_ms
    }

    fn warmup_complete(&self) -> bool {
        self.warmup_target > INITIAL_COUNTER_U64 && self.warmup_count >= self.warmup_target
    }

    fn apply_selection_timing(&mut self, snapshot: &RuntimeSelectionSnapshot) {
        match &snapshot.decision.state {
            SelectionState::Active { market } => {
                self.selection_published_at_ms = Some(snapshot.published_at_ms);
                self.seconds_to_expiry_at_selection = Some(market.seconds_to_end);
            }
            #[cfg(test)]
            SelectionState::Freeze { market, .. } => {
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

    fn observe_reference_quote(&mut self, quote: &FastSpotObservation) {
        if self.phase == SelectionPhase::Idle {
            return;
        }
        let Some(interval_start_ms) = self.interval_start_ms else {
            return;
        };
        let Some(anchor_price) = self.price_to_beat.or(Some(quote.price)) else {
            return;
        };
        if quote.observed_ts_ms < interval_start_ms {
            return;
        }
        if self
            .last_reference_ts_ms
            .is_some_and(|last_ts_ms| quote.observed_ts_ms <= last_ts_ms)
        {
            return;
        }

        self.last_reference_ts_ms = Some(quote.observed_ts_ms);
        if self.interval_open.is_none() {
            self.interval_open = Some(anchor_price);
        }
        self.warmup_count += COUNTER_INCREMENT as u64;
    }

    #[cfg(test)]
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

pub struct BinaryOracleEdgeTaker {
    core: StrategyCore,
    config: BinaryOracleEdgeTakerConfig,
    context: StrategyBuildContext,
    active: ActiveMarketState,
    book_subscriptions: OutcomeBookSubscriptions,
    market_lifecycle: BTreeMap<String, MarketLifecycleLedger>,
    exposure: ExposureState,
    last_reported_exposure_occupancy: Cell<Option<ExposureOccupancy>>,
    pricing: PricingState,
    selection_missing_since_ms: Option<u64>,
    #[cfg(test)]
    book_subscription_events: Vec<BookSubscriptionEvent>,
}

impl BinaryOracleEdgeTaker {
    fn new(config: BinaryOracleEdgeTakerConfig, context: StrategyBuildContext) -> Self {
        let pricing = PricingState::from_config(&config);
        let oms_type = parse_configured_oms_type(CONFIG_FIELD_OMS_TYPE, &config.oms_type)
            .expect("validated binary_oracle_edge_taker oms_type");
        let market_exit_time_in_force = parse_configured_time_in_force(
            CONFIG_FIELD_MARKET_EXIT_TIME_IN_FORCE,
            &config.market_exit_time_in_force,
        )
        .expect("validated binary_oracle_edge_taker market_exit_time_in_force");
        let external_order_claims = config
            .external_order_claims
            .iter()
            .map(|instrument_id| InstrumentId::from(instrument_id.as_str()))
            .collect::<Vec<_>>();
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(StrategyId::from(config.strategy_id.as_str())),
                order_id_tag: Some(config.order_id_tag.clone()),
                use_uuid_client_order_ids: config.use_uuid_client_order_ids,
                use_hyphens_in_client_order_ids: config.use_hyphens_in_client_order_ids,
                oms_type: Some(oms_type),
                external_order_claims: Some(external_order_claims),
                manage_contingent_orders: config.manage_contingent_orders,
                manage_gtd_expiry: config.manage_gtd_expiry,
                manage_stop: config.manage_stop,
                market_exit_interval_ms: config.market_exit_interval_ms,
                market_exit_max_attempts: config.market_exit_max_attempts,
                market_exit_time_in_force,
                market_exit_reduce_only: config.market_exit_reduce_only,
                log_events: config.log_events,
                log_commands: config.log_commands,
                log_rejected_due_post_only_as_warning: config.log_rejected_due_post_only_as_warning,
            }),
            config,
            context,
            active: ActiveMarketState::idle(),
            book_subscriptions: OutcomeBookSubscriptions::empty(),
            market_lifecycle: BTreeMap::new(),
            exposure: ExposureState::Flat,
            last_reported_exposure_occupancy: Cell::new(None),
            pricing,
            selection_missing_since_ms: None,
            #[cfg(test)]
            book_subscription_events: Vec::new(),
        }
    }

    fn apply_selection_snapshot(&mut self, snapshot: RuntimeSelectionSnapshot) {
        let now_ms = snapshot.published_at_ms;
        let previous_active = self.active.clone();
        let previous_phase = previous_active.phase;
        let previous_fee_instrument_ids = previous_active.outcome_fees.instrument_ids();
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
        let next_fee_instrument_ids = self.active.outcome_fees.instrument_ids();
        if previous_fee_instrument_ids != next_fee_instrument_ids
            || (same_market_interval_rollover && !next_fee_instrument_ids.is_empty())
            || (reactivated_into_active && !next_fee_instrument_ids.is_empty())
        {
            self.trigger_fee_warm_for_market();
            self.refresh_fee_readiness();
        }
        self.sync_exposure_context_from_active();
        self.prune_market_lifecycle(now_ms);
        self.refresh_book_subscriptions_for_current_state();
        if self.exposure.managed_position().is_some()
            && let Err(error) = self.try_submit_exit_order(now_ms)
        {
            log::error!(
                "binary_oracle_edge_taker exit submit failed on selection update: strategy_id={} market_id={:?} now_ms={} error={:#}",
                self.config.strategy_id,
                self.active.market_id,
                now_ms,
                error,
            );
        }
    }

    fn observe_reference_quote(&mut self, quote: &FastSpotObservation) {
        self.active.observe_reference_quote(quote);
        self.pricing.observe_reference_quote(
            quote,
            self.config.lead_agreement_min_corr,
            self.config.lead_jitter_max_ms,
        );
        self.active.fast_venue_incoherent = self.pricing.fast_venue_incoherent;
        self.refresh_fee_readiness();
        self.sync_exposure_context_from_active();
        if self.exposure.managed_position().is_some()
            && let Err(error) = self.try_submit_exit_order(quote.observed_ts_ms)
        {
            log::error!(
                "binary_oracle_edge_taker exit submit failed on reference update: strategy_id={} market_id={:?} ts_ms={} error={:#}",
                self.config.strategy_id,
                self.active.market_id,
                quote.observed_ts_ms,
                error,
            );
        }
    }

    #[cfg(test)]
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
                "binary_oracle_edge_taker exit submit failed on reference update: strategy_id={} market_id={:?} ts_ms={} error={:#}",
                self.config.strategy_id,
                self.active.market_id,
                snapshot.ts_ms,
                error,
            );
        }
    }

    fn reference_quote_from_tick(&self, quote: &QuoteTick) -> Option<FastSpotObservation> {
        let bid = quote.bid_price.as_f64();
        let ask = quote.ask_price.as_f64();
        if !is_positive_finite(bid) || !is_positive_finite(ask) {
            return None;
        }
        let midpoint = (bid + ask) / MIDPOINT_DIVISOR_F64;
        if !is_positive_finite(midpoint) {
            return None;
        }
        let observed_ts_ms = quote.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        Some(FastSpotObservation {
            venue_name: self.config.reference_venue.clone(),
            price: midpoint,
            observed_ts_ms,
        })
    }

    fn refresh_fee_readiness(&mut self) {
        refresh_fee_readiness_for_active(&mut self.active, self.context.fee_provider());
    }

    fn trigger_fee_warm_for_market(&self) {
        let instrument_ids = self.active.outcome_fees.instrument_ids();
        if instrument_ids.is_empty() {
            return;
        }
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for instrument_id in instrument_ids {
            let fee_provider = self.context.fee_provider_arc();
            handle.spawn(async move {
                let _ = fee_provider.warm(instrument_id).await;
            });
        }
    }

    fn selection_retry_timer_name(&self) -> String {
        format!("{}:selection_retry", self.config.strategy_id)
    }

    fn register_selection_retry_timer(&mut self) {
        let timer_name = self.selection_retry_timer_name();
        let strategy_id = self.config.strategy_id.clone();
        let interval_ns = self
            .config
            .retry_interval_seconds
            .saturating_mul(NANOS_PER_SECOND_U64);
        if let Err(error) =
            self.clock()
                .set_timer_ns(&timer_name, interval_ns, None, None, None, None, None)
        {
            log::error!(
                "binary_oracle_edge_taker selection retry timer registration failed: strategy_id={} error={:#}",
                strategy_id,
                error,
            );
        }
    }

    fn deregister_selection_retry_timer(&mut self) {
        let timer_name = self.selection_retry_timer_name();
        self.clock().cancel_timer(timer_name.as_str());
        self.replace_book_subscriptions(OutcomeBookSubscriptions::empty());
    }

    fn refresh_selection_from_cache(&mut self, now_ms: u64) {
        let instruments = {
            let cache = self.cache();
            cache
                .instrument_ids(None)
                .into_iter()
                .filter_map(|instrument_id| cache.instrument(instrument_id).cloned())
                .collect::<Vec<_>>()
        };
        let snapshot = selection_snapshot_from_instruments(&self.config, &instruments, now_ms);
        if matches!(snapshot.decision.state, SelectionState::Idle { .. }) {
            if self.selection_missing_since_ms.is_none() {
                self.selection_missing_since_ms = Some(now_ms);
            }
            let missing_since_ms = self
                .selection_missing_since_ms
                .expect("selection_missing_since_ms set before blocked-target check");
            let blocked_after_ms = self
                .config
                .blocked_after_seconds
                .saturating_mul(MILLIS_PER_SECOND_U64);
            if now_ms.saturating_sub(missing_since_ms) >= blocked_after_ms {
                self.apply_selection_snapshot(idle_selection_snapshot(
                    &self.config,
                    now_ms,
                    SELECTION_BLOCK_REASON_TARGET_SELECTION_BLOCKED,
                ));
                return;
            }
        } else {
            self.selection_missing_since_ms = None;
        }
        self.apply_selection_snapshot(snapshot);
    }

    fn reference_instrument_id(&self) -> InstrumentId {
        InstrumentId::from(self.config.reference_instrument_id.as_str())
    }

    fn subscribe_reference_quotes(&mut self) {
        let instrument_id = self.reference_instrument_id();
        #[cfg(not(test))]
        self.subscribe_quotes(instrument_id, None, None);
        #[cfg(test)]
        let _ = instrument_id;
    }

    fn unsubscribe_reference_quotes(&mut self) {
        let instrument_id = self.reference_instrument_id();
        #[cfg(not(test))]
        self.unsubscribe_quotes(instrument_id, None, None);
        #[cfg(test)]
        let _ = instrument_id;
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
                    outcome_side: None,
                    outcome_fees: OutcomeFeeState::empty(),
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
                    "binary_oracle_edge_taker recovery probe could not access cache: strategy_id={} entering fail-closed recovery mode",
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
                "binary_oracle_edge_taker recovery bootstrap found multiple open positions: strategy_id={} position_count={} leaving recovery mode blind to position bootstrap",
                self.config.strategy_id,
                cached_positions.len(),
            );
            return;
        }

        let open_position = cached_positions
            .into_iter()
            .next()
            .expect("checked non-empty recovery position set");
        if self
            .configured_position_contract()
            .ok()
            .is_some_and(|contract| {
                supports_strategy_managed_position(
                    open_position.entry_order_side,
                    open_position.side,
                    contract,
                )
            })
        {
            self.exposure = ExposureState::Managed(ManagedPositionState {
                position: open_position.clone(),
                origin: ManagedPositionOrigin::RecoveryBootstrap,
            });
            log::warn!(
                "binary_oracle_edge_taker recovery bootstrap loaded cached open position: strategy_id={} position_id={} instrument_id={} entry_order_side={:?} side={:?} quantity={} avg_px_open={}",
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
                "binary_oracle_edge_taker recovery bootstrap quarantined unsupported cached position: strategy_id={} position_id={} instrument_id={} entry_order_side={:?} side={:?} quantity={} avg_px_open={}",
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
                "binary_oracle_edge_taker recovery bootstrap received invalid cached position side: strategy_id={} position_id={} instrument_id={} entry_order_side={:?} side={:?}",
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
            let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
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
            .or_insert_with(MarketLifecycleLedger::empty)
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
            .or_insert_with(MarketLifecycleLedger::empty);
        ledger.churn_count = ledger.churn_count.saturating_add(COUNTER_INCREMENT_U64);
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
            last_reference_ts_ms: self.active.last_reference_ts_ms,
            now_ms,
            stale_reference_after_ms: self.config.forced_flat_stale_reference_ms,
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
            last_reference_ts_ms: self.active.last_reference_ts_ms,
            now_ms,
            stale_reference_after_ms: self.config.forced_flat_stale_reference_ms,
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
            .filter(|value| is_positive_finite(*value));
        if spot_price.is_none() {
            blocked_by.push(EntryPricingBlockReason::SpotPriceMissing);
        }

        let strike_price = self
            .active
            .interval_open
            .filter(|value| is_positive_finite(*value));
        if strike_price.is_none() {
            blocked_by.push(EntryPricingBlockReason::StrikePriceMissing);
        }

        let seconds_to_expiry = self.current_seconds_to_expiry_at(now_ms);
        if seconds_to_expiry.is_none() {
            blocked_by.push(EntryPricingBlockReason::SecondsToExpiryMissing);
        }

        let realized_vol = self
            .current_realized_vol_at(now_ms)
            .filter(|value| is_positive_finite(*value));
        if realized_vol.is_none() {
            blocked_by.push(EntryPricingBlockReason::RealizedVolNotReady);
        }

        let theta_scaled_min_edge_bps = seconds_to_expiry.and_then(|seconds_to_expiry| {
            compute_theta_scaler(&ThetaScalerInputs {
                seconds_to_expiry,
                cadence_seconds: self.config.cadence_seconds,
                theta_decay_factor: self.config.theta_decay_factor,
            })
            .map(|theta| self.config.edge_threshold_basis_points as f64 * theta)
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

    fn current_position_fast_spot(&self) -> Option<&FastSpotObservation> {
        let open_position = &self.managed_position()?.position;
        if open_position.market_id.as_deref() != self.active.market_id.as_deref() {
            return None;
        }
        self.pricing.fast_spot.as_ref()
    }

    fn current_position_spot_price(&self) -> Option<f64> {
        self.current_position_fast_spot()
            .map(|spot| spot.price)
            .filter(|value| is_positive_finite(*value))
    }

    fn current_scaled_min_edge_bps_at(&self, now_ms: u64) -> Option<f64> {
        compute_theta_scaler(&ThetaScalerInputs {
            seconds_to_expiry: self.current_seconds_to_expiry_at(now_ms)?,
            cadence_seconds: self.config.cadence_seconds,
            theta_decay_factor: self.config.theta_decay_factor,
        })
        .map(|theta| self.config.edge_threshold_basis_points as f64 * theta)
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
        let time_uncertainty_probability = if self.config.cadence_seconds == 0 {
            return None;
        } else {
            clamp_probability(
                UNIT_F64 - seconds_to_expiry as f64 / self.config.cadence_seconds as f64,
            )
        };
        let fee_uncertainty_probability =
            clamp_probability(up_fee_bps.max(down_fee_bps) / BPS_DENOMINATOR);
        let lead_gap_probability = self.pricing.last_lead_gap_probability?;
        let jitter_penalty_probability = self.pricing.last_jitter_penalty_probability?;

        uncertainty_band_probability(&UncertaintyBandInputs {
            lead_gap_probability,
            jitter_penalty_probability,
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
            fair_probability_down: evaluation.fair_probability_up.map(|value| UNIT_F64 - value),
            uncertainty_band_probability: evaluation.uncertainty_band_probability,
            uncertainty_band_live: evaluation.uncertainty_band_probability.is_some(),
            uncertainty_band_reason: if evaluation.uncertainty_band_probability.is_some() {
                EVIDENCE_REASON_DERIVED_FROM_LEAD_GAP_JITTER_TIME_AND_FEE
            } else {
                EVIDENCE_REASON_UNCERTAINTY_BAND_UNAVAILABLE
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
            expected_ev_per_notional: evaluation.expected_ev_per_notional,
            maximum_position_notional: self.config.maximum_position_notional,
            risk_lambda: self.config.risk_lambda,
            book_impact_cap_bps: self.config.book_impact_cap_bps,
            book_impact_cap_notional: evaluation.book_impact_cap_notional,
            sized_notional: evaluation.sized_notional,
            selected_side: evaluation.selected_side,
            fast_venue_available,
            reference_fair_value_available_without_fast_venue: !fast_venue_available
                && self.pricing.last_reference_fair_value.is_some(),
            lead_quality_policy_applied: self.pricing.lead_quality_policy_applied,
            lead_quality_reason: if self.pricing.fast_venue_incoherent {
                EVIDENCE_REASON_NO_FAST_VENUE_CLEARED_LEAD_QUALITY_THRESHOLDS
            } else {
                EVIDENCE_REASON_LEAD_QUALITY_THRESHOLDS_APPLIED_TO_LIVE_FAST_SPOT_SELECTION
            },
            final_fee_amount_known: false,
            final_fee_amount_reason:
                EVIDENCE_REASON_FINAL_FEE_REQUIRES_SIDE_PRICE_AND_SIZE_SELECTION,
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
                "binary_oracle_edge_taker entry blocked: strategy_id={} reasons={:?}",
                self.config.strategy_id,
                fields.gate_blocked_by
            );
            if fields
                .gate_blocked_by
                .contains(&EntryBlockReason::FeesNotReady)
            {
                log::warn!(
                    "binary_oracle_edge_taker fee-rate unavailable: strategy_id={} entry remains fail-closed",
                    self.config.strategy_id
                );
            }
            log::warn!(
                "binary_oracle_edge_taker entry evaluation: strategy_id={} market_id={:?} phase={:?} gate_blocked_by={:?} pricing_blocked_by={:?} spot_price={:?} spot_venue_name={:?} reference_fair_value={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} theta_decay_factor={} theta_scaled_min_edge_bps={:?} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} uncertainty_band_live={} uncertainty_band_reason={} lead_agreement_corr={:?} fast_venue_age_ms={:?} fast_venue_jitter_ms={:?} up_fee_bps={:?} down_fee_bps={:?} up_entry_cost={:?} down_entry_cost={:?} up_worst_case_ev_bps={:?} down_worst_case_ev_bps={:?} expected_ev_per_notional={:?} maximum_position_notional={} risk_lambda={} book_impact_cap_bps={} book_impact_cap_notional={:?} sized_notional={:?} selected_side={:?} fast_venue_available={} reference_fair_value_available_without_fast_venue={} lead_quality_policy_applied={} lead_quality_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity_value={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
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
                fields.expected_ev_per_notional,
                fields.maximum_position_notional,
                fields.risk_lambda,
                fields.book_impact_cap_bps,
                fields.book_impact_cap_notional,
                fields.sized_notional,
                fields.selected_side,
                fields.fast_venue_available,
                fields.reference_fair_value_available_without_fast_venue,
                fields.lead_quality_policy_applied,
                fields.lead_quality_reason,
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
                "binary_oracle_edge_taker entry evaluation: strategy_id={} market_id={:?} phase={:?} gate_blocked_by={:?} pricing_blocked_by={:?} spot_price={:?} spot_venue_name={:?} reference_fair_value={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} theta_decay_factor={} theta_scaled_min_edge_bps={:?} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} uncertainty_band_live={} uncertainty_band_reason={} lead_agreement_corr={:?} fast_venue_age_ms={:?} fast_venue_jitter_ms={:?} up_fee_bps={:?} down_fee_bps={:?} up_entry_cost={:?} down_entry_cost={:?} up_worst_case_ev_bps={:?} down_worst_case_ev_bps={:?} expected_ev_per_notional={:?} maximum_position_notional={} risk_lambda={} book_impact_cap_bps={} book_impact_cap_notional={:?} sized_notional={:?} selected_side={:?} fast_venue_available={} reference_fair_value_available_without_fast_venue={} lead_quality_policy_applied={} lead_quality_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity_value={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
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
                fields.expected_ev_per_notional,
                fields.maximum_position_notional,
                fields.risk_lambda,
                fields.book_impact_cap_bps,
                fields.book_impact_cap_notional,
                fields.sized_notional,
                fields.selected_side,
                fields.fast_venue_available,
                fields.reference_fair_value_available_without_fast_venue,
                fields.lead_quality_policy_applied,
                fields.lead_quality_reason,
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

    fn outcome_fee_bps(&self, side: OutcomeSide) -> Option<f64> {
        let instrument_id = match side {
            OutcomeSide::Up => self.active.outcome_fees.up_instrument_id,
            OutcomeSide::Down => self.active.outcome_fees.down_instrument_id,
        }?;
        self.context.fee_provider().fee_bps(instrument_id)?.to_f64()
    }

    fn active_book_for_outcome(&self, side: OutcomeSide) -> &OutcomeBookState {
        match side {
            OutcomeSide::Up => &self.active.books.up,
            OutcomeSide::Down => &self.active.books.down,
        }
    }

    fn configured_entry_order_side(&self) -> Result<OrderSide> {
        parse_configured_order_side(CONFIG_FIELD_ENTRY_ORDER_SIDE, &self.config.entry_order.side)
    }

    fn executable_entry_cost(&self, side: OutcomeSide) -> Option<f64> {
        let order_side = self.configured_entry_order_side().ok()?;
        self.active_book_for_outcome(side)
            .executable_price_for_order_side(order_side)
    }

    fn submission_entry_price(&self, side: OutcomeSide) -> Option<f64> {
        self.executable_entry_cost(side)
    }

    fn visible_book_notional_cap(&self, side: OutcomeSide) -> Option<f64> {
        let order_side = self.configured_entry_order_side().ok()?;
        let capped_execution = self
            .active_book_for_outcome(side)
            .max_execution_within_vwap_slippage_bps(order_side, self.config.book_impact_cap_bps)
            .filter(|execution| is_positive_finite(execution.quantity))?;
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
                .or_else(|| pending_context.and_then(|pending| pending.market_id.clone())),
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
                .filter(|_| {
                    self.configured_position_contract()
                        .ok()
                        .is_some_and(|contract| {
                            supports_strategy_managed_position(
                                spec.entry_order_side,
                                spec.side,
                                contract,
                            )
                        })
                }),
            outcome_fees: preserved
                .map(|position| position.outcome_fees.clone())
                .or_else(|| pending_context.map(|pending| pending.outcome_fees.clone()))
                .unwrap_or_else(OutcomeFeeState::empty),
            historical_entry_fee_bps: preserved
                .and_then(|position| position.historical_entry_fee_bps)
                .or_else(|| pending_context.and_then(|pending| pending.historical_entry_fee_bps)),
            entry_order_side: spec.entry_order_side,
            side: spec.side,
            quantity: spec.quantity,
            avg_px_open: spec.avg_px_open,
            interval_open: preserved
                .and_then(|position| position.interval_open)
                .or_else(|| pending_context.and_then(|pending| pending.interval_open)),
            selection_published_at_ms: preserved
                .and_then(|position| position.selection_published_at_ms)
                .or_else(|| pending_context.and_then(|pending| pending.selection_published_at_ms)),
            seconds_to_expiry_at_selection: preserved
                .and_then(|position| position.seconds_to_expiry_at_selection)
                .or_else(|| {
                    pending_context.and_then(|pending| pending.seconds_to_expiry_at_selection)
                }),
            book: match (
                preserved.map(|position| position.book.clone()),
                pending_context.map(|pending| pending.book.clone()),
            ) {
                (Some(book), _) | (None, Some(book)) => book,
                (None, None) => OutcomeBookState::from_instrument_id(spec.instrument_id),
            },
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
            self.configured_position_contract()
                .ok()
                .is_some_and(|contract| {
                    supports_strategy_managed_position(entry_order_side, side, contract)
                });

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
                        side: Some(side),
                    },
                })
            };
            log::error!(
                "binary_oracle_edge_taker position event carried unsupported position side: strategy_id={} instrument_id={} position_id={} entry_order_side={:?} side={:?}",
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
                "binary_oracle_edge_taker quarantining unsupported observed position contract: strategy_id={} instrument_id={} entry_order_side={:?} side={:?}",
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

        let origin = match self
            .managed_position()
            .filter(|managed| {
                managed.position.position_id == position_id
                    && managed.position.instrument_id == instrument_id
            })
            .map(|managed| managed.origin)
        {
            Some(origin) => origin,
            None if pending_matches => ManagedPositionOrigin::StrategyEntry,
            None => ManagedPositionOrigin::RecoveryBootstrap,
        };
        let materialized_position = self.build_open_position_state(
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
        );
        self.exposure = match self.exposure.exit_pending().cloned() {
            Some(exit_pending)
                if exit_pending.position.as_ref().is_some_and(|managed| {
                    managed.position.position_id == position_id
                        && managed.position.instrument_id == instrument_id
                }) =>
            {
                ExposureState::ExitPending(ExitPendingState {
                    position: Some(ManagedPositionState {
                        position: materialized_position,
                        origin,
                    }),
                    pending_exit: exit_pending.pending_exit,
                })
            }
            _ => ExposureState::Managed(ManagedPositionState {
                position: materialized_position,
                origin,
            }),
        };
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

    fn configured_position_contract(&self) -> Result<ConfiguredPositionContract> {
        Ok(ConfiguredPositionContract {
            entry_order_side: parse_configured_order_side(
                CONFIG_FIELD_ENTRY_ORDER_SIDE,
                &self.config.entry_order.side,
            )?,
            entry_position_side: parse_configured_position_side(
                CONFIG_FIELD_ENTRY_ORDER_POSITION_SIDE,
                &self.config.entry_order.position_side,
            )?,
            exit_order_side: parse_configured_order_side(
                CONFIG_FIELD_EXIT_ORDER_SIDE,
                &self.config.exit_order.side,
            )?,
            exit_position_side: parse_configured_position_side(
                CONFIG_FIELD_EXIT_ORDER_POSITION_SIDE,
                &self.config.exit_order.position_side,
            )?,
        })
    }

    fn open_position_effective_entry_cost(&self) -> Option<f64> {
        let open_position = &self.managed_position()?.position;
        let contract = self.configured_position_contract().ok()?;
        managed_position_effective_entry_cost(
            open_position,
            contract.entry_order_side,
            contract.entry_position_side,
        )
    }

    fn current_exit_order_for_open_position(&self) -> Option<(OrderSide, f64)> {
        let open_position = &self.managed_position()?.position;
        let contract = self.configured_position_contract().ok()?;
        managed_position_exit_order(
            open_position,
            contract.exit_order_side,
            contract.exit_position_side,
        )
    }

    fn current_exit_value_for_open_position(&self) -> Option<f64> {
        let open_position = &self.managed_position()?.position;
        let contract = self.configured_position_contract().ok()?;
        managed_position_exit_value(
            open_position,
            contract.exit_order_side,
            contract.exit_position_side,
        )
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
        let spot_price = self.current_position_spot_price()?;
        let strike_price = open_position
            .interval_open
            .filter(|value| is_positive_finite(*value))?;
        let seconds_to_expiry = self.current_position_seconds_to_expiry_at(now_ms)?;
        let realized_vol = self
            .current_realized_vol_at(now_ms)
            .filter(|value| is_positive_finite(*value))?;
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
        let spot_price = self.current_position_spot_price()?;
        let strike_price = open_position
            .interval_open
            .filter(|value| is_positive_finite(*value))?;
        let seconds_to_expiry = Self::seconds_to_expiry_from_selection(
            open_position.selection_published_at_ms,
            open_position.seconds_to_expiry_at_selection,
            now_ms,
        )?;
        let realized_vol = self
            .current_realized_vol_at(now_ms)
            .filter(|value| is_positive_finite(*value))?;
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
            effective_entry_cost * (UNIT_F64 + historical_entry_fee_bps / BPS_DENOMINATOR);
        if !is_positive_finite(total_entry_cost) {
            return None;
        }

        let current_exit_value = self.current_exit_value_for_open_position()?;
        let net_exit_value =
            current_exit_value * (UNIT_F64 - current_exit_fee_bps / BPS_DENOMINATOR);
        if !is_positive_finite(net_exit_value) {
            return None;
        }

        Some(((net_exit_value - total_entry_cost) / total_entry_cost) * BPS_DENOMINATOR)
    }

    fn open_position_historical_entry_fee_bps(&self) -> Option<f64> {
        self.managed_position()?.position.historical_entry_fee_bps
    }

    fn historical_entry_fee_log_fields(&self) -> (bool, &'static str) {
        let Some(managed_position) = self.managed_position() else {
            return (false, EVIDENCE_REASON_NO_MANAGED_POSITION);
        };

        if managed_position.position.historical_entry_fee_bps.is_some() {
            (true, EVIDENCE_REASON_CAPTURED_FROM_STRATEGY_ENTRY_STATE)
        } else if managed_position.origin == ManagedPositionOrigin::RecoveryBootstrap {
            (
                false,
                EVIDENCE_REASON_RECOVERY_BOOTSTRAP_POSITION_MISSING_ORIGINAL_FEE_RATE,
            )
        } else {
            (
                false,
                EVIDENCE_REASON_POSITION_STATE_MISSING_ORIGINAL_FEE_RATE,
            )
        }
    }

    fn position_outcome_fee_bps(&self, side: OutcomeSide) -> Option<f64> {
        let open_position = &self.managed_position()?.position;
        let instrument_id = match side {
            OutcomeSide::Up => open_position.outcome_fees.up_instrument_id,
            OutcomeSide::Down => open_position.outcome_fees.down_instrument_id,
        }?;
        self.context.fee_provider().fee_bps(instrument_id)?.to_f64()
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
            evaluation.blocked_reason = Some(EXIT_BLOCK_REASON_NO_OPEN_POSITION);
            return evaluation;
        }
        if self.exposure.exit_pending().is_some() {
            evaluation.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_ALREADY_PENDING);
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
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_DECISION_UNAVAILABLE);
            return decision;
        };
        if exit_decision == ExitDecision::Hold {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_HOLD);
            return decision;
        }

        let Some(open_position) = self.managed_position().map(|managed| &managed.position) else {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_OPEN_POSITION_MISSING);
            return decision;
        };
        let Some((order_side, price)) = self.current_exit_order_for_open_position() else {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_PRICE_MISSING);
            return decision;
        };
        if !is_positive_finite(open_position.quantity.as_f64()) {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_QUANTITY_NOT_POSITIVE);
            return decision;
        }

        decision.instrument_id = Some(open_position.instrument_id);
        decision.order_side = Some(order_side);
        decision.price = Some(price);
        decision.quantity = Some(open_position.quantity);
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
            spot_price: self.current_position_spot_price(),
            spot_venue_name: self
                .current_position_fast_spot()
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
                .map(|value| UNIT_F64 - value),
            uncertainty_band_probability: self
                .current_position_uncertainty_band_probability_at(now_ms),
            up_fee_bps: self.position_outcome_fee_bps(OutcomeSide::Up),
            down_fee_bps: self.position_outcome_fee_bps(OutcomeSide::Down),
            hold_ev_bps: decision.evaluation.hold_ev_bps,
            exit_ev_bps: decision.evaluation.exit_ev_bps,
            exit_decision: decision.evaluation.exit_decision,
            historical_entry_fee_rate_known,
            historical_entry_fee_rate_reason,
            final_fee_amount_known: false,
            final_fee_amount_reason:
                EVIDENCE_REASON_FINAL_FEE_REQUIRES_SIDE_PRICE_SIZE_AND_ACTUAL_FILL,
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
                    "binary_oracle_edge_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_fair_value={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} up_fee_bps={:?} down_fee_bps={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} historical_entry_fee_rate_known={} historical_entry_fee_rate_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
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
                    "binary_oracle_edge_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_fair_value={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} up_fee_bps={:?} down_fee_bps={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} historical_entry_fee_rate_known={} historical_entry_fee_rate_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
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
                "binary_oracle_edge_taker exit evaluation: strategy_id={} market_id={:?} phase={:?} position_outcome_side={:?} position_id={:?} position_instrument_id={:?} position_quantity={:?} position_avg_px_open={:?} forced_flat_reasons={:?} spot_price={:?} spot_venue_name={:?} reference_fair_value={:?} interval_open={:?} seconds_to_expiry={:?} realized_vol={:?} realized_vol_source_venue={:?} realized_vol_source_ts_ms={:?} pricing_kurtosis={} exit_hysteresis_bps={} fair_probability_up={:?} fair_probability_down={:?} uncertainty_band_probability={:?} up_fee_bps={:?} down_fee_bps={:?} hold_ev_bps={:?} exit_ev_bps={:?} exit_decision={:?} historical_entry_fee_rate_known={} historical_entry_fee_rate_reason={} final_fee_amount_known={} final_fee_amount_reason={} submission_instrument_id={:?} submission_order_side={:?} submission_price={:?} submission_quantity={:?} submission_client_order_id={:?} submission_blocked_reason={:?}",
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

    fn submit_order_with_decision_evidence(
        &mut self,
        intent: BoltV3OrderIntentEvidence,
        order: nautilus_model::orders::OrderAny,
        client_id: ClientId,
    ) -> Result<()> {
        self.context
            .decision_evidence()
            .record_order_intent(&intent)?;
        let request = submit_admission_request_from_intent(&intent)?;
        let _permit = self.context.submit_admission().admit(&request)?;
        self.submit_order(order, None, Some(client_id))
    }

    fn build_configured_entry_order(
        &mut self,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
        client_order_id: ClientOrderId,
    ) -> Result<nautilus_model::orders::OrderAny> {
        build_configured_order(
            &mut self.core,
            ORDER_CONFIGURATION_PREFIX_ENTRY,
            &self.config.entry_order.order_type,
            &self.config.entry_order.time_in_force,
            self.config.entry_order.is_post_only,
            self.config.entry_order.is_reduce_only,
            self.config.entry_order.is_quote_quantity,
            instrument_id,
            order_side,
            quantity,
            price,
            client_order_id,
        )
    }

    fn build_configured_exit_order(
        &mut self,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
        client_order_id: ClientOrderId,
    ) -> Result<nautilus_model::orders::OrderAny> {
        build_configured_order(
            &mut self.core,
            ORDER_CONFIGURATION_PREFIX_EXIT,
            &self.config.exit_order.order_type,
            &self.config.exit_order.time_in_force,
            self.config.exit_order.is_post_only,
            self.config.exit_order.is_reduce_only,
            self.config.exit_order.is_quote_quantity,
            instrument_id,
            order_side,
            quantity,
            price,
            client_order_id,
        )
    }

    fn try_submit_exit_order(&mut self, now_ms: u64) -> Result<Option<ClientOrderId>> {
        let mut decision = self.exit_submission_decision_at(now_ms);

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
        let order = self.build_configured_exit_order(
            instrument_id,
            order_side,
            quantity,
            price,
            client_order_id,
        )?;

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
            "binary_oracle_edge_taker exit submit: strategy_id={} instrument_id={} order_side={:?} price={} quantity={} client_order_id={}",
            self.config.strategy_id,
            instrument_id,
            order_side,
            price,
            quantity,
            client_order_id,
        );

        let intent = BoltV3OrderIntentEvidence {
            strategy_id: self.config.strategy_id.clone(),
            intent_kind: BoltV3OrderIntentKind::Exit,
            instrument_id: instrument_id.to_string(),
            client_order_id: client_order_id.to_string(),
            order_side: order_side.to_string(),
            price: price.to_string(),
            quantity: quantity.to_string(),
        };

        if let Err(error) = self.submit_order_with_decision_evidence(intent, order, client_id) {
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
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_STRATEGY_CORE_NOT_REGISTERED);
            return decision;
        }

        if !evaluation.gate.blocked_by.is_empty() {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_ENTRY_GATE_BLOCKED);
            return decision;
        }
        if !evaluation.pricing_blocked_by.is_empty() {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_ENTRY_PRICING_BLOCKED);
            return decision;
        }

        let Some(selected_side) = evaluation.selected_side else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_NO_SIDE_SELECTED);
            return decision;
        };
        let Some(sized_notional) = evaluation
            .sized_notional
            .filter(|value| is_positive_finite(*value))
        else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_SIZED_NOTIONAL_NOT_POSITIVE);
            return decision;
        };

        let Some(instrument_id) = self.instrument_id_for_side(selected_side) else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_INSTRUMENT_ID_MISSING);
            return decision;
        };
        let Some(instrument) = self.current_instrument(instrument_id) else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_INSTRUMENT_MISSING_FROM_CACHE);
            return decision;
        };
        let Some(price) = self.submission_entry_price(selected_side) else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_ENTRY_PRICE_MISSING);
            return decision;
        };
        let Some(entry_cost) = self.executable_entry_cost(selected_side) else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_ENTRY_COST_MISSING);
            return decision;
        };
        let shares_value = sized_notional / entry_cost;
        let Ok(quantity) = instrument.try_make_qty(shares_value, Some(true)) else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_QUANTITY_ROUNDING_FAILED);
            return decision;
        };
        let quantity_value = quantity.as_f64();
        if !is_positive_finite(quantity_value) {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_QUANTITY_NOT_POSITIVE);
            return decision;
        }

        let Ok(contract) = self.configured_position_contract() else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_POSITION_CONTRACT_INVALID);
            return decision;
        };
        let order_side = contract.entry_order_side;
        let position_side = contract.entry_position_side;
        if !supports_strategy_managed_position(order_side, position_side, contract) {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_ENTRY_POSITION_CONTRACT_UNSUPPORTED);
            return decision;
        }

        decision.instrument_id = Some(instrument_id);
        decision.order_side = Some(order_side);
        decision.price = Some(price);
        decision.quantity_value = Some(quantity_value);
        decision
    }

    fn try_submit_entry_order(&mut self, now_ms: u64) -> Result<Option<ClientOrderId>> {
        let decision = self.entry_submission_decision_at(now_ms);
        self.log_entry_evaluation(now_ms, &decision);

        let Some(instrument_id) = decision.instrument_id else {
            if let Some(reason) = decision.blocked_reason {
                log::warn!(
                    "binary_oracle_edge_taker entry submit skipped: strategy_id={} reason={}",
                    self.config.strategy_id,
                    reason
                );
            }
            return Ok(None);
        };
        let Some(order_side) = decision.order_side else {
            if let Some(reason) = decision.blocked_reason {
                log::warn!(
                    "binary_oracle_edge_taker entry submit skipped: strategy_id={} reason={}",
                    self.config.strategy_id,
                    reason
                );
            }
            return Ok(None);
        };
        let Some(price) = decision.price else {
            if let Some(reason) = decision.blocked_reason {
                log::warn!(
                    "binary_oracle_edge_taker entry submit skipped: strategy_id={} reason={}",
                    self.config.strategy_id,
                    reason
                );
            }
            return Ok(None);
        };
        let Some(quantity_value) = decision.quantity_value else {
            if let Some(reason) = decision.blocked_reason {
                log::warn!(
                    "binary_oracle_edge_taker entry submit skipped: strategy_id={} reason={}",
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
                "binary_oracle_edge_taker entry submit skipped: strategy_id={} reason=historical_entry_fee_unavailable",
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
        let order = self.build_configured_entry_order(
            instrument_id,
            order_side,
            quantity,
            price,
            client_order_id,
        )?;

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
            "binary_oracle_edge_taker entry submit: strategy_id={} instrument_id={} order_side={:?} price={} quantity={} client_order_id={}",
            self.config.strategy_id,
            instrument_id,
            order_side,
            price,
            quantity,
            client_order_id,
        );

        let intent = BoltV3OrderIntentEvidence {
            strategy_id: self.config.strategy_id.clone(),
            intent_kind: BoltV3OrderIntentKind::Entry,
            instrument_id: instrument_id.to_string(),
            client_order_id: client_order_id.to_string(),
            order_side: order_side.to_string(),
            price: price.to_string(),
            quantity: quantity.to_string(),
        };

        if let Err(error) = self.submit_order_with_decision_evidence(intent, order, client_id) {
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
            expected_ev_per_notional: None,
            book_impact_cap_notional: None,
            sized_notional: None,
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
            let expected_ev_per_notional =
                selected_worst_case_ev_bps.map(|ev_bps| ev_bps / BPS_DENOMINATOR);
            let book_impact_cap_notional = self.visible_book_notional_cap(selected_side);
            evaluation.expected_ev_per_notional = expected_ev_per_notional;
            evaluation.book_impact_cap_notional = book_impact_cap_notional;
            if let (Some(expected_ev_per_notional), Some(book_impact_cap_notional)) =
                (expected_ev_per_notional, book_impact_cap_notional)
            {
                evaluation.sized_notional = Some(choose_robust_size(&RobustSizingInputs {
                    expected_ev_per_notional,
                    risk_lambda: self.config.risk_lambda,
                    order_notional_target: self.config.order_notional_target,
                    maximum_position_notional: self.config.maximum_position_notional,
                    impact_cap_notional: book_impact_cap_notional,
                }));
            }
        }
        evaluation
    }
}

impl std::fmt::Debug for BinaryOracleEdgeTaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinaryOracleEdgeTaker")
            .field("config", &self.config)
            .finish()
    }
}

impl DataActor for BinaryOracleEdgeTaker {
    fn on_start(&mut self) -> Result<()> {
        self.bootstrap_recovery_from_cache();
        let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
        self.refresh_selection_from_cache(now_ms);
        self.register_selection_retry_timer();
        self.subscribe_reference_quotes();
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        self.unsubscribe_reference_quotes();
        self.deregister_selection_retry_timer();
        Ok(())
    }

    fn on_time_event(&mut self, event: &TimeEvent) -> Result<()> {
        if event.name.as_str() == self.selection_retry_timer_name() {
            self.refresh_selection_from_cache(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
        }
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> anyhow::Result<()> {
        if quote.instrument_id != self.reference_instrument_id() {
            return Ok(());
        }
        if let Some(reference_quote) = self.reference_quote_from_tick(quote) {
            self.observe_reference_quote(&reference_quote);
        }
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

        let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
        if matches!(self.exposure, ExposureState::Managed(_))
            && let Err(error) = self.try_submit_exit_order(now_ms)
        {
            log::error!(
                "binary_oracle_edge_taker exit submit failed on book delta: strategy_id={} instrument_id={} error={:#}",
                self.config.strategy_id,
                deltas.instrument_id,
                error
            );
        }
        if self.exposure_occupancy().is_none()
            && let Err(error) = self.try_submit_entry_order(now_ms)
        {
            log::error!(
                "binary_oracle_edge_taker entry submit failed on book delta: strategy_id={} instrument_id={} error={:#}",
                self.config.strategy_id,
                deltas.instrument_id,
                error
            );
        }
        Ok(())
    }

    fn on_order_filled(
        &mut self,
        event: &nautilus_model::events::OrderFilled,
    ) -> anyhow::Result<()> {
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        let entry_fill = self
            .pending_entry()
            .is_some_and(|pending| pending.client_order_id == event.client_order_id);
        let exit_fill = self
            .exposure
            .exit_pending()
            .is_some_and(|exit| exit.pending_exit.client_order_id == event.client_order_id);

        if entry_fill {
            let pending_context = self.pending_entry_context_for(event.instrument_id);
            let position_side = self
                .configured_position_contract()
                .ok()
                .and_then(|contract| {
                    infer_strategy_position_side_from_entry_fill(
                        event.order_side,
                        contract.entry_order_side,
                        contract.entry_position_side,
                    )
                });
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
                            side: position_side,
                        },
                    });
                }
                log::error!(
                    "binary_oracle_edge_taker entry fill could not materialize configured position contract: strategy_id={} client_order_id={} instrument_id={} order_side={:?} position_id_present={} position_side_inferable={}",
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
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
        Ok(())
    }
}

nautilus_strategy!(BinaryOracleEdgeTaker, {
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
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
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
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
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

    fn on_position_closed(&mut self, event: nautilus_model::events::PositionClosed) {
        match &mut self.exposure {
            ExposureState::Managed(position)
                if position.position.position_id == event.position_id =>
            {
                self.exposure = ExposureState::Flat;
            }
            ExposureState::ExitPending(exit_pending)
                if exit_pending.pending_exit.position_id == Some(event.position_id) =>
            {
                exit_pending.pending_exit.close_received = true;
                exit_pending.position = None;
                if exit_pending.is_terminal() {
                    self.exposure = ExposureState::Flat;
                }
            }
            ExposureState::UnsupportedObserved(observed)
                if observed.observed.position_id == event.position_id =>
            {
                self.exposure = ExposureState::Flat;
            }
            // Entry reconciliation may not have a position id yet; the instrument is the
            // strongest available key for a close that races ahead of position materialization.
            ExposureState::EntryReconcilePending { pending, .. }
                if pending.instrument_id == event.instrument_id =>
            {
                self.exposure = ExposureState::Flat;
            }
            _ => {}
        }
        self.refresh_book_subscriptions_for_current_state();
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
    }
});

#[derive(Debug)]
pub struct BinaryOracleEdgeTakerBuilder;

pub const KEY: &str = stringify!(binary_oracle_edge_taker);
const ENTRY_ORDER_FIELD: &str = stringify!(entry_order);
const EXIT_ORDER_FIELD: &str = stringify!(exit_order);
const WRONG_TYPE_CODE: &str = stringify!(wrong_type);
const UNKNOWN_FIELD_CODE: &str = stringify!(unknown_field);
const TARGET_MARKET_NOT_FOUND_REASON: &str = stringify!(target_market_not_found);

impl BinaryOracleEdgeTakerBuilder {
    fn parse_config(raw: &Value) -> Result<BinaryOracleEdgeTakerConfig> {
        raw.clone()
            .try_into()
            .context("binary_oracle_edge_taker builder requires a valid config table")
    }

    fn push_missing(
        errors: &mut Vec<ValidationError>,
        field: String,
        code: &'static str,
        field_type: BinaryOracleEdgeTakerFieldType,
    ) {
        errors.push(ValidationError {
            field,
            code,
            message: format!("is missing required {} field", field_type.expected()),
        });
    }

    fn push_wrong_type(
        errors: &mut Vec<ValidationError>,
        field: String,
        field_type: BinaryOracleEdgeTakerFieldType,
        value: &Value,
    ) {
        errors.push(ValidationError {
            field,
            code: WRONG_TYPE_CODE,
            message: format!(
                "must be {} {}, got {} value",
                field_type.article(),
                field_type.expected(),
                value.type_str()
            ),
        });
    }

    fn push_unknown_field(errors: &mut Vec<ValidationError>, field: String, key: &str) {
        errors.push(ValidationError {
            field,
            code: UNKNOWN_FIELD_CODE,
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
                ENTRY_ORDER_FIELD
                    | EXIT_ORDER_FIELD
                    | binary_oracle_edge_taker_config_fields!(match_config_field_names)
            ) {
                Self::push_unknown_field(errors, format!("{field_prefix}.{key}"), key);
            }
        }

        binary_oracle_edge_taker_config_fields!(validate_config_fields_impl)(
            table,
            field_prefix,
            errors,
        );
        Self::validate_order_table(
            table,
            field_prefix,
            ENTRY_ORDER_FIELD,
            concat!(stringify!(missing_), stringify!(entry_order)),
            errors,
        );
        Self::validate_order_table(
            table,
            field_prefix,
            EXIT_ORDER_FIELD,
            concat!(stringify!(missing_), stringify!(exit_order)),
            errors,
        );
    }

    fn validate_order_table(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        field_name: &'static str,
        missing_code: &'static str,
        errors: &mut Vec<ValidationError>,
    ) {
        let field = format!("{field_prefix}.{field_name}");
        let Some(value) = table.get(field_name) else {
            Self::push_missing(
                errors,
                field,
                missing_code,
                BinaryOracleEdgeTakerFieldType::Table,
            );
            return;
        };
        let Some(order_table) = value.as_table() else {
            Self::push_wrong_type(errors, field, BinaryOracleEdgeTakerFieldType::Table, value);
            return;
        };

        for key in order_table.keys() {
            if !matches!(
                key.as_str(),
                binary_oracle_edge_taker_order_fields!(match_order_field_names)
            ) {
                Self::push_unknown_field(errors, format!("{field}.{key}"), key);
            }
        }

        binary_oracle_edge_taker_order_fields!(validate_order_fields_impl)(
            order_table,
            &field,
            errors,
        );
    }

    fn validate_order_field(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        field_name: &'static str,
        missing_code: &'static str,
        field_type: BinaryOracleEdgeTakerFieldType,
        errors: &mut Vec<ValidationError>,
    ) {
        let field = format!("{field_prefix}.{field_name}");
        match table.get(field_name) {
            None => Self::push_missing(errors, field, missing_code, field_type),
            Some(value) if !field_type.matches(value) => {
                Self::push_wrong_type(errors, field, field_type, value);
            }
            Some(_) => {}
        }
    }
}

impl StrategyBuilder for BinaryOracleEdgeTakerBuilder {
    fn kind() -> &'static str {
        KEY
    }

    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
        let Some(table) = raw.as_table() else {
            Self::push_wrong_type(
                errors,
                field_prefix.to_string(),
                BinaryOracleEdgeTakerFieldType::Table,
                raw,
            );
            return;
        };

        Self::validate_table(table, field_prefix, errors);
    }

    fn build(raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        Ok(Box::new(BinaryOracleEdgeTaker::new(
            Self::parse_config(raw)?,
            context.clone(),
        )))
    }

    fn register(
        raw: &Value,
        context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId> {
        let strategy = BinaryOracleEdgeTaker::new(Self::parse_config(raw)?, context.clone());
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
        SelectionState::Active { market } => OutcomeBookSubscriptions::from_market(market),
        #[cfg(test)]
        SelectionState::Freeze { market, .. } => OutcomeBookSubscriptions::from_market(market),
        SelectionState::Idle { .. } => OutcomeBookSubscriptions::empty(),
    }
}

fn selection_snapshot_from_instruments(
    config: &BinaryOracleEdgeTakerConfig,
    instruments: &[InstrumentAny],
    now_ms: u64,
) -> RuntimeSelectionSnapshot {
    let Some(market) = select_configured_market_from_instruments(config, instruments, now_ms)
    else {
        return idle_selection_snapshot(config, now_ms, TARGET_MARKET_NOT_FOUND_REASON);
    };
    selection_snapshot_for_state(config, now_ms, SelectionState::Active { market })
}

fn idle_selection_snapshot(
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

fn selection_snapshot_for_state(
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

fn select_configured_market_from_instruments(
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
        price_to_beat: None,
        start_ts_ms: market.start_timestamp_milliseconds,
        seconds_to_end: market.seconds_to_end,
    })
}

fn should_replace_book_subscriptions(
    current: &OutcomeBookSubscriptions,
    next: &OutcomeBookSubscriptions,
) -> bool {
    !current.is_same_market(next)
}

fn unsubscribe_missing_books(
    strategy: &mut BinaryOracleEdgeTaker,
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
    strategy: &mut BinaryOracleEdgeTaker,
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

const BOOK_SUBSCRIBE_ACTION: &str = stringify!(subscribe);
const BOOK_UNSUBSCRIBE_ACTION: &str = stringify!(unsubscribe);

impl BookSubscriptionEvent {
    fn subscribe(instrument_id: InstrumentId) -> Self {
        Self {
            action: BOOK_SUBSCRIBE_ACTION,
            instrument_id,
        }
    }

    fn unsubscribe(instrument_id: InstrumentId) -> Self {
        Self {
            action: BOOK_UNSUBSCRIBE_ACTION,
            instrument_id,
        }
    }
}

impl BinaryOracleEdgeTaker {
    fn record_book_subscription_event(&mut self, event: BookSubscriptionEvent) {
        #[cfg(test)]
        self.book_subscription_events.push(event);
        #[cfg(not(test))]
        let _ = event;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfiguredOrderType {
    Limit,
    Market,
}

const ORDER_SIDE_BUY_VALUE: &str = stringify!(buy);
const ORDER_SIDE_SELL_VALUE: &str = stringify!(sell);
const POSITION_SIDE_LONG_VALUE: &str = stringify!(long);
const POSITION_SIDE_SHORT_VALUE: &str = stringify!(short);
const ORDER_TYPE_LIMIT_VALUE: &str = stringify!(limit);
const ORDER_TYPE_MARKET_VALUE: &str = stringify!(market);
const TIME_IN_FORCE_GTC_VALUE: &str = stringify!(gtc);
const TIME_IN_FORCE_FOK_VALUE: &str = stringify!(fok);
const TIME_IN_FORCE_IOC_VALUE: &str = stringify!(ioc);
const OMS_TYPE_NETTING_VALUE: &str = stringify!(netting);

fn parse_configured_order_side(field: &str, value: &str) -> Result<OrderSide> {
    match value {
        ORDER_SIDE_BUY_VALUE => Ok(OrderSide::Buy),
        ORDER_SIDE_SELL_VALUE => Ok(OrderSide::Sell),
        _ => anyhow::bail!("{field} must be `buy` or `sell`, got `{value}`"),
    }
}

fn parse_configured_position_side(field: &str, value: &str) -> Result<PositionSide> {
    match value {
        POSITION_SIDE_LONG_VALUE => Ok(PositionSide::Long),
        POSITION_SIDE_SHORT_VALUE => Ok(PositionSide::Short),
        _ => anyhow::bail!("{field} must be `long` or `short`, got `{value}`"),
    }
}

fn parse_configured_order_type(field: &str, value: &str) -> Result<ConfiguredOrderType> {
    match value {
        ORDER_TYPE_LIMIT_VALUE => Ok(ConfiguredOrderType::Limit),
        ORDER_TYPE_MARKET_VALUE => Ok(ConfiguredOrderType::Market),
        _ => anyhow::bail!("{field} must be `limit` or `market`, got `{value}`"),
    }
}

fn parse_configured_time_in_force(field: &str, value: &str) -> Result<TimeInForce> {
    match value {
        TIME_IN_FORCE_GTC_VALUE => Ok(TimeInForce::Gtc),
        TIME_IN_FORCE_FOK_VALUE => Ok(TimeInForce::Fok),
        TIME_IN_FORCE_IOC_VALUE => Ok(TimeInForce::Ioc),
        _ => anyhow::bail!("{field} must be `gtc`, `fok`, or `ioc`, got `{value}`"),
    }
}

fn parse_configured_oms_type(field: &str, value: &str) -> Result<NtOmsType> {
    match value {
        OMS_TYPE_NETTING_VALUE => Ok(NtOmsType::Netting),
        _ => anyhow::bail!("{field} must be `netting`, got `{value}`"),
    }
}

#[expect(clippy::too_many_arguments)]
fn build_configured_order(
    core: &mut StrategyCore,
    prefix: &'static str,
    order_type: &str,
    time_in_force: &str,
    is_post_only: bool,
    is_reduce_only: bool,
    is_quote_quantity: bool,
    instrument_id: InstrumentId,
    order_side: OrderSide,
    quantity: Quantity,
    price: Price,
    client_order_id: ClientOrderId,
) -> Result<nautilus_model::orders::OrderAny> {
    let order_type = parse_configured_order_type(&format!("{prefix}_order_type"), order_type)?;
    let time_in_force =
        parse_configured_time_in_force(&format!("{prefix}_time_in_force"), time_in_force)?;
    match order_type {
        ConfiguredOrderType::Limit => Ok(core.order_factory().limit(
            instrument_id,
            order_side,
            quantity,
            price,
            Some(time_in_force),
            None,
            Some(is_post_only),
            Some(is_reduce_only),
            Some(is_quote_quantity),
            None,
            None,
            None,
            None,
            None,
            None,
            Some(client_order_id),
        )),
        ConfiguredOrderType::Market => {
            anyhow::ensure!(
                !is_post_only,
                "{prefix}_is_post_only must be false for market orders"
            );
            Ok(core.order_factory().market(
                instrument_id,
                order_side,
                quantity,
                Some(time_in_force),
                Some(is_reduce_only),
                Some(is_quote_quantity),
                None,
                None,
                None,
                Some(client_order_id),
            ))
        }
    }
}

fn refresh_fee_readiness_for_active(
    active: &mut ActiveMarketState,
    fee_provider: &dyn FeeProvider,
) {
    active.outcome_fees.up_ready = active
        .outcome_fees
        .up_instrument_id
        .and_then(|instrument_id| fee_provider.fee_bps(instrument_id))
        .is_some();
    active.outcome_fees.down_ready = active
        .outcome_fees
        .down_instrument_id
        .and_then(|instrument_id| fee_provider.fee_bps(instrument_id))
        .is_some();
}

const ZERO_F64: f64 = 0.0;
const UNIT_F64: f64 = 1.0;
const INITIAL_COUNTER_U64: u64 = 0;
const INITIAL_COUNTER_USIZE: usize = 0;
const MIN_OBSERVATION_COUNT: u64 = 1;
const COUNTER_INCREMENT: usize = 1;
const COUNTER_INCREMENT_U64: u64 = 1;
const POWER_OF_TWO: i32 = 2;
const BPS_DENOMINATOR: f64 = 10_000.0;
const MIDPOINT_DIVISOR_F64: f64 = 2.0;
const QUADRATIC_RISK_DIVISOR: f64 = 2.0;
const MILLIS_PER_SECOND_U64: u64 = 1_000;
const MILLIS_PER_SECOND_F64: f64 = 1_000.0;
const NANOS_PER_MILLI_U64: u64 = 1_000_000;
const NANOS_PER_SECOND_U64: u64 = 1_000_000_000;
const DAYS_PER_YEAR_F64: f64 = 365.25;
const HOURS_PER_DAY_F64: f64 = 24.0;
const MINUTES_PER_HOUR_F64: f64 = 60.0;
const SECONDS_PER_MINUTE_F64: f64 = 60.0;
const SECONDS_PER_YEAR_F64: f64 =
    DAYS_PER_YEAR_F64 * HOURS_PER_DAY_F64 * MINUTES_PER_HOUR_F64 * SECONDS_PER_MINUTE_F64;
const KURTOSIS_NORMALIZATION: f64 = 6.0;
const NORMAL_DENSITY_EXPONENT_DIVISOR: f64 = 2.0;
const NORMAL_CDF_T_SCALE: f64 = 0.231_641_9;
const NORMAL_CDF_DENSITY_SCALE: f64 = 0.398_942_3;
const NORMAL_CDF_POLY_A1: f64 = 0.319_381_5;
const NORMAL_CDF_POLY_A2: f64 = -0.356_563_8;
const NORMAL_CDF_POLY_A3: f64 = 1.781_478;
const NORMAL_CDF_POLY_A4: f64 = -1.821_256;
const NORMAL_CDF_POLY_A5: f64 = 1.330_274;
const CONFIG_FIELD_OMS_TYPE: &str = "oms_type";
const CONFIG_FIELD_MARKET_EXIT_TIME_IN_FORCE: &str = "market_exit_time_in_force";
const CONFIG_FIELD_ENTRY_ORDER_SIDE: &str = "entry_order_side";
const CONFIG_FIELD_ENTRY_ORDER_POSITION_SIDE: &str = "entry_order_position_side";
const CONFIG_FIELD_EXIT_ORDER_SIDE: &str = "exit_order_side";
const CONFIG_FIELD_EXIT_ORDER_POSITION_SIDE: &str = "exit_order_position_side";
const ORDER_CONFIGURATION_PREFIX_ENTRY: &str = "entry";
const ORDER_CONFIGURATION_PREFIX_EXIT: &str = "exit";
const SELECTION_BLOCK_REASON_TARGET_SELECTION_BLOCKED: &str = "target_selection_blocked";
const EVIDENCE_REASON_DERIVED_FROM_LEAD_GAP_JITTER_TIME_AND_FEE: &str =
    "derived_from_lead_gap_jitter_time_and_fee";
const EVIDENCE_REASON_UNCERTAINTY_BAND_UNAVAILABLE: &str = "uncertainty_band_unavailable";
const EVIDENCE_REASON_NO_FAST_VENUE_CLEARED_LEAD_QUALITY_THRESHOLDS: &str =
    "no_fast_venue_cleared_lead_quality_thresholds";
const EVIDENCE_REASON_LEAD_QUALITY_THRESHOLDS_APPLIED_TO_LIVE_FAST_SPOT_SELECTION: &str =
    "lead_quality_thresholds_applied_to_live_fast_spot_selection";
const EVIDENCE_REASON_FINAL_FEE_REQUIRES_SIDE_PRICE_AND_SIZE_SELECTION: &str =
    "final_fee_requires_side_price_and_size_selection";
const EVIDENCE_REASON_FINAL_FEE_REQUIRES_SIDE_PRICE_SIZE_AND_ACTUAL_FILL: &str =
    "final_fee_requires_side_price_size_and_actual_fill";
const EVIDENCE_REASON_NO_MANAGED_POSITION: &str = "no_managed_position";
const EVIDENCE_REASON_CAPTURED_FROM_STRATEGY_ENTRY_STATE: &str =
    "captured_from_strategy_entry_state";
const EVIDENCE_REASON_RECOVERY_BOOTSTRAP_POSITION_MISSING_ORIGINAL_FEE_RATE: &str =
    "recovery_bootstrap_position_missing_original_fee_rate";
const EVIDENCE_REASON_POSITION_STATE_MISSING_ORIGINAL_FEE_RATE: &str =
    "position_state_missing_original_fee_rate";
const ENTRY_BLOCK_REASON_STRATEGY_CORE_NOT_REGISTERED: &str = "strategy_core_not_registered";
const ENTRY_BLOCK_REASON_ENTRY_GATE_BLOCKED: &str = "entry_gate_blocked";
const ENTRY_BLOCK_REASON_ENTRY_PRICING_BLOCKED: &str = "entry_pricing_blocked";
const ENTRY_BLOCK_REASON_NO_SIDE_SELECTED: &str = "no_side_selected";
const ENTRY_BLOCK_REASON_SIZED_NOTIONAL_NOT_POSITIVE: &str = "sized_notional_not_positive";
const ENTRY_BLOCK_REASON_INSTRUMENT_ID_MISSING: &str = "instrument_id_missing";
const ENTRY_BLOCK_REASON_INSTRUMENT_MISSING_FROM_CACHE: &str = "instrument_missing_from_cache";
const ENTRY_BLOCK_REASON_ENTRY_PRICE_MISSING: &str = "entry_price_missing";
const ENTRY_BLOCK_REASON_ENTRY_COST_MISSING: &str = "entry_cost_missing";
const ENTRY_BLOCK_REASON_QUANTITY_ROUNDING_FAILED: &str = "quantity_rounding_failed";
const ENTRY_BLOCK_REASON_QUANTITY_NOT_POSITIVE: &str = "quantity_not_positive";
const ENTRY_BLOCK_REASON_POSITION_CONTRACT_INVALID: &str = "position_contract_invalid";
const ENTRY_BLOCK_REASON_ENTRY_POSITION_CONTRACT_UNSUPPORTED: &str =
    "entry_position_contract_unsupported";
const EXIT_BLOCK_REASON_NO_OPEN_POSITION: &str = "no_open_position";
const EXIT_BLOCK_REASON_EXIT_ALREADY_PENDING: &str = "exit_already_pending";
const EXIT_BLOCK_REASON_EXIT_DECISION_UNAVAILABLE: &str = "exit_decision_unavailable";
const EXIT_BLOCK_REASON_EXIT_HOLD: &str = "exit_hold";
const EXIT_BLOCK_REASON_OPEN_POSITION_MISSING: &str = "open_position_missing";
const EXIT_BLOCK_REASON_EXIT_PRICE_MISSING: &str = "exit_price_missing";
const EXIT_BLOCK_REASON_EXIT_QUANTITY_NOT_POSITIVE: &str = "exit_quantity_not_positive";

fn is_positive_finite(value: f64) -> bool {
    value.is_finite() && value > ZERO_F64
}

fn is_non_negative_finite(value: f64) -> bool {
    value.is_finite() && value >= ZERO_F64
}

fn clamp_probability(value: f64) -> f64 {
    value.clamp(ZERO_F64, UNIT_F64)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutcomeSide {
    Up,
    Down,
}

#[cfg(test)]
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

fn price_agreement_corr(observed_price: f64, anchor_price: f64) -> Option<f64> {
    if !is_positive_finite(observed_price) || !is_positive_finite(anchor_price) {
        return None;
    }
    Some(clamp_probability(
        UNIT_F64 - ((observed_price - anchor_price).abs() / anchor_price),
    ))
}

fn price_gap_probability(observed_price: f64, reference_price: f64) -> Option<f64> {
    if !is_positive_finite(observed_price) || !is_positive_finite(reference_price) {
        return None;
    }
    Some(clamp_probability(
        (observed_price - reference_price).abs() / reference_price,
    ))
}

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
fn best_healthy_oracle_price(snapshot: &ReferenceSnapshot) -> Option<f64> {
    snapshot
        .venues
        .iter()
        .filter(|venue| {
            venue.venue_kind == VenueKind::Oracle
                && !venue.stale
                && matches!(venue.health, VenueHealth::Healthy)
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
        || !is_positive_finite(inputs.spot_price)
        || !is_positive_finite(inputs.strike_price)
        || !is_positive_finite(inputs.realized_vol)
        || !inputs.pricing_kurtosis.is_finite()
    {
        return None;
    }

    let sigma_eff =
        inputs.realized_vol * (UNIT_F64 + inputs.pricing_kurtosis / KURTOSIS_NORMALIZATION);
    if !is_positive_finite(sigma_eff) {
        return None;
    }

    let time_to_expiry_years = inputs.seconds_to_expiry as f64 / SECONDS_PER_YEAR_F64;
    if time_to_expiry_years <= ZERO_F64 {
        return None;
    }

    let d2 = ((inputs.spot_price / inputs.strike_price).ln()
        - (sigma_eff.powi(POWER_OF_TWO) / QUADRATIC_RISK_DIVISOR) * time_to_expiry_years)
        / (sigma_eff * time_to_expiry_years.sqrt());
    sanitize_probability(standard_normal_cdf(d2))
}

fn standard_normal_cdf(x: f64) -> f64 {
    let t = UNIT_F64 / (UNIT_F64 + NORMAL_CDF_T_SCALE * x.abs());
    let d = NORMAL_CDF_DENSITY_SCALE * (-x * x / NORMAL_DENSITY_EXPONENT_DIVISOR).exp();
    let prob = d
        * t
        * (NORMAL_CDF_POLY_A1
            + t * (NORMAL_CDF_POLY_A2
                + t * (NORMAL_CDF_POLY_A3 + t * (NORMAL_CDF_POLY_A4 + t * NORMAL_CDF_POLY_A5))));
    if x > ZERO_F64 { UNIT_F64 - prob } else { prob }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ThetaScalerInputs {
    seconds_to_expiry: u64,
    cadence_seconds: u64,
    theta_decay_factor: f64,
}

fn compute_theta_scaler(inputs: &ThetaScalerInputs) -> Option<f64> {
    if !is_non_negative_finite(inputs.theta_decay_factor) {
        return None;
    }
    if inputs.theta_decay_factor == ZERO_F64 {
        return Some(UNIT_F64);
    }
    if inputs.cadence_seconds == 0 {
        return None;
    }

    let ratio = clamp_probability(inputs.seconds_to_expiry as f64 / inputs.cadence_seconds as f64);
    Some(UNIT_F64 + inputs.theta_decay_factor * (UNIT_F64 - ratio).powi(POWER_OF_TWO))
}

fn compute_worst_case_ev_bps(side: OutcomeSide, inputs: &WorstCaseEvInputs) -> Option<f64> {
    let fair_probability = sanitize_probability(inputs.fair_probability?)?;
    let uncertainty_band_probability = sanitize_probability(inputs.uncertainty_band_probability)?;
    let executable_entry_cost = inputs.executable_entry_cost;
    let fee_bps = inputs.fee_bps?;

    if !is_positive_finite(executable_entry_cost) {
        return None;
    }
    if !is_non_negative_finite(fee_bps) {
        return None;
    }

    let p_lo = clamp_probability(fair_probability - uncertainty_band_probability);
    let p_hi = clamp_probability(fair_probability + uncertainty_band_probability);
    let worst_case_success_probability = match side {
        OutcomeSide::Up => p_lo,
        OutcomeSide::Down => UNIT_F64 - p_hi,
    };
    let total_entry_cost = executable_entry_cost * (UNIT_F64 + fee_bps / BPS_DENOMINATOR);

    if total_entry_cost <= ZERO_F64 {
        return None;
    }

    Some(((worst_case_success_probability - total_entry_cost) / total_entry_cost) * BPS_DENOMINATOR)
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
    expected_ev_per_notional: f64,
    risk_lambda: f64,
    order_notional_target: f64,
    maximum_position_notional: f64,
    impact_cap_notional: f64,
}

fn choose_robust_size(inputs: &RobustSizingInputs) -> f64 {
    if !is_positive_finite(inputs.expected_ev_per_notional) {
        return ZERO_F64;
    }

    let cap = sanitize_non_negative(inputs.order_notional_target)
        .min(sanitize_non_negative(inputs.maximum_position_notional))
        .min(sanitize_non_negative(inputs.impact_cap_notional));
    if cap <= ZERO_F64 {
        return ZERO_F64;
    }

    if !is_non_negative_finite(inputs.risk_lambda) {
        return ZERO_F64;
    }
    if inputs.risk_lambda == ZERO_F64 {
        return cap;
    }

    (inputs.expected_ev_per_notional / (QUADRATIC_RISK_DIVISOR * inputs.risk_lambda)).min(cap)
}

fn sanitize_probability(value: f64) -> Option<f64> {
    if value.is_finite() && (ZERO_F64..=UNIT_F64).contains(&value) {
        Some(value)
    } else {
        None
    }
}

fn sanitize_non_negative(value: f64) -> f64 {
    if value.is_finite() {
        value.max(ZERO_F64)
    } else {
        ZERO_F64
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
    StaleReference,
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
    expected_ev_per_notional: Option<f64>,
    book_impact_cap_notional: Option<f64>,
    sized_notional: Option<f64>,
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
    expected_ev_per_notional: Option<f64>,
    maximum_position_notional: f64,
    risk_lambda: f64,
    book_impact_cap_bps: u64,
    book_impact_cap_notional: Option<f64>,
    sized_notional: Option<f64>,
    selected_side: Option<OutcomeSide>,
    fast_venue_available: bool,
    reference_fair_value_available_without_fast_venue: bool,
    lead_quality_policy_applied: bool,
    lead_quality_reason: &'static str,
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
    order_side: OrderSide,
    quantity: Quantity,
    price_precision: u8,
    time_in_force: TimeInForce,
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
    let raw_price = match inputs.order_side {
        OrderSide::Buy => inputs.best_ask,
        OrderSide::Sell => inputs.best_bid,
        _ => anyhow::bail!(
            "entry order side must be `buy` or `sell`, got `{:?}`",
            inputs.order_side
        ),
    };
    anyhow::ensure!(
        raw_price.is_finite() && raw_price > 0.0,
        "entry price must be positive"
    );

    Ok(EntryOrderPlan {
        client_order_id: inputs.client_order_id,
        instrument_id: inputs.instrument_id,
        order_side: inputs.order_side,
        quantity: inputs.quantity,
        price: Price::new(raw_price, inputs.price_precision),
        time_in_force: inputs.time_in_force,
    })
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

const NO_OPEN_POSITION_REASON: &str = stringify!(no_open_position);
const EXIT_ALREADY_PENDING_REASON: &str = stringify!(exit_already_pending);
const EXIT_HOLD_REASON: &str = stringify!(exit_hold);

fn should_warn_on_exit_submission_block(reason: Option<&str>) -> bool {
    !matches!(reason, Some(reason) if reason == NO_OPEN_POSITION_REASON
        || reason == EXIT_ALREADY_PENDING_REASON
        || reason == EXIT_HOLD_REASON)
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
    last_reference_ts_ms: Option<u64>,
    now_ms: u64,
    stale_reference_after_ms: u64,
    liquidity_available: Option<f64>,
    min_liquidity_required: f64,
    fast_venue_incoherent: bool,
}

fn evaluate_forced_flat_predicates(inputs: &ForcedFlatInputs) -> Vec<ForcedFlatReason> {
    let mut reasons = Vec::new();
    let reference_stale = inputs.last_reference_ts_ms.is_some_and(|last_ts_ms| {
        inputs.now_ms.saturating_sub(last_ts_ms) > inputs.stale_reference_after_ms
    });

    if inputs.phase == SelectionPhase::Freeze {
        reasons.push(ForcedFlatReason::Freeze);
    }
    if reference_stale {
        reasons.push(ForcedFlatReason::StaleReference);
    }
    if inputs
        .liquidity_available
        .is_none_or(|liquidity| !liquidity.is_finite() || liquidity < inputs.min_liquidity_required)
    {
        reasons.push(ForcedFlatReason::ThinBook);
    }
    if !inputs.metadata_matches_selection {
        reasons.push(ForcedFlatReason::MetadataMismatch);
    }
    if inputs.fast_venue_incoherent && reference_stale {
        reasons.push(ForcedFlatReason::FastVenueIncoherent);
    }

    reasons
}

fn submit_admission_request_from_intent(
    intent: &BoltV3OrderIntentEvidence,
) -> Result<BoltV3SubmitAdmissionRequest> {
    let price = Decimal::from_str(intent.price.trim()).with_context(|| {
        format!(
            "bolt-v3 submit admission price is not a decimal for client_order_id={}",
            intent.client_order_id
        )
    })?;
    let quantity = Decimal::from_str(intent.quantity.trim()).with_context(|| {
        format!(
            "bolt-v3 submit admission quantity is not a decimal for client_order_id={}",
            intent.client_order_id
        )
    })?;

    Ok(BoltV3SubmitAdmissionRequest {
        strategy_id: intent.strategy_id.clone(),
        client_order_id: intent.client_order_id.clone(),
        instrument_id: intent.instrument_id.clone(),
        notional: price * quantity,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use anyhow::Result;
    use futures_util::future::{BoxFuture, FutureExt};
    use nautilus_common::{cache::Cache, clock::TestClock};
    use nautilus_core::{Params, UnixNanos};
    use nautilus_model::{
        enums::AssetClass,
        identifiers::{Symbol, TraderId},
        instruments::BinaryOption,
        types::{Currency, Price, Quantity},
    };
    use nautilus_portfolio::portfolio::Portfolio;
    use rust_decimal::Decimal;

    use super::*;
    use crate::strategies::{production_strategy_registry, registry::StrategyBuilder};

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
            strategy_id = "BINARYORACLEEDGETAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
            client_id = "POLYMARKET"
            configured_target_id = "btc_updown_5m"
            target_kind = "rotating_market"
            rotating_market_family = "updown"
            underlying_asset = "BTC"
            cadence_seconds = 300
            cadence_slug_token = "5m"
            market_selection_rule = "active_or_next"
            retry_interval_seconds = 5
            blocked_after_seconds = 60
            reference_venue = "binance_reference"
            reference_instrument_id = "BTCUSDT.BINANCE"
            use_uuid_client_order_ids = true
            use_hyphens_in_client_order_ids = false
            external_order_claims = ["ETHUSDT.BINANCE"]
            manage_contingent_orders = true
            manage_gtd_expiry = true
            manage_stop = true
            market_exit_interval_ms = 250
            market_exit_max_attempts = 7
            market_exit_time_in_force = "ioc"
            market_exit_reduce_only = false
            log_events = false
            log_commands = false
            log_rejected_due_post_only_as_warning = false
            warmup_tick_count = 20
            reentry_cooldown_secs = 30
            order_notional_target = 1000.0
            maximum_position_notional = 1000.0
            book_impact_cap_bps = 15
            risk_lambda = 0.5
            edge_threshold_basis_points = -20
            exit_hysteresis_bps = 5
            vol_window_secs = 60
            vol_gap_reset_secs = 10
            vol_min_observations = 20
            vol_bridge_valid_secs = 10
            pricing_kurtosis = 0.0
            theta_decay_factor = 0.0
            forced_flat_stale_reference_ms = 1500
            forced_flat_thin_book_min_liquidity = 100.0
            lead_agreement_min_corr = 0.8
            lead_jitter_max_ms = 250

            [entry_order]
            side = "buy"
            position_side = "long"
            order_type = "limit"
            time_in_force = "fok"
            is_post_only = false
            is_reduce_only = false
            is_quote_quantity = false

            [exit_order]
            side = "sell"
            position_side = "long"
            order_type = "market"
            time_in_force = "ioc"
            is_post_only = false
            is_reduce_only = false
            is_quote_quantity = false
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

        fn set_fee(&self, instrument_id: &str, fee_bps: Decimal) {
            self.fees
                .lock()
                .expect("recording fee provider mutex poisoned")
                .insert(instrument_id.to_string(), fee_bps);
        }

        fn warm_calls(&self) -> Vec<String> {
            self.warm_calls
                .lock()
                .expect("recording fee provider mutex poisoned")
                .clone()
        }
    }

    impl FeeProvider for RecordingFeeProvider {
        fn fee_bps(&self, instrument_id: InstrumentId) -> Option<Decimal> {
            self.fees
                .lock()
                .expect("recording fee provider mutex poisoned")
                .get(instrument_id.to_string().as_str())
                .copied()
        }

        fn warm(&self, instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
            self.warm_calls
                .lock()
                .expect("recording fee provider mutex poisoned")
                .push(instrument_id.to_string());
            async { Ok(()) }.boxed()
        }
    }

    #[derive(Debug)]
    struct RecordingDecisionEvidenceWriter;

    impl crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter
        for RecordingDecisionEvidenceWriter
    {
        fn record_order_intent(
            &self,
            _intent: &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_admission_decision(
            &self,
            _decision: &crate::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FailingDecisionEvidenceWriter;

    impl crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter
        for FailingDecisionEvidenceWriter
    {
        fn record_order_intent(
            &self,
            _intent: &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence,
        ) -> Result<()> {
            anyhow::bail!("intent write failed")
        }

        fn record_admission_decision(
            &self,
            _decision: &crate::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence,
        ) -> Result<()> {
            anyhow::bail!("admission decision write failed")
        }
    }

    fn test_strategy() -> BinaryOracleEdgeTaker {
        test_strategy_with_fee_provider(RecordingFeeProvider::cold())
    }

    fn register_test_strategy(strategy: &mut BinaryOracleEdgeTaker) -> Rc<RefCell<Cache>> {
        let clock = Rc::new(RefCell::new(TestClock::new()));
        clock
            .borrow_mut()
            .set_time(UnixNanos::from(1_200_u64 * NANOS_PER_MILLI_U64));
        let cache = Rc::new(RefCell::new(Cache::default()));
        let cache_handle = cache.clone();
        let portfolio = Rc::new(RefCell::new(Portfolio::new(
            cache.clone(),
            clock.clone(),
            None,
        )));
        strategy
            .core
            .register(TraderId::from("TRADER-001"), clock, cache, portfolio)
            .expect("test strategy should register with NT core");
        cache_handle
    }

    fn register_test_strategy_with_active_instruments(strategy: &mut BinaryOracleEdgeTaker) {
        let cache = register_test_strategy(strategy);
        add_active_instruments_to_cache(strategy, &cache);
    }

    fn add_active_instruments_to_cache(
        strategy: &BinaryOracleEdgeTaker,
        cache: &Rc<RefCell<Cache>>,
    ) {
        let up_instrument_id = strategy
            .active
            .books
            .up
            .instrument_id
            .expect("test strategy must have active up instrument");
        let down_instrument_id = strategy
            .active
            .books
            .down
            .instrument_id
            .expect("test strategy must have active down instrument");
        let up_instrument_id = up_instrument_id.to_string();
        let down_instrument_id = down_instrument_id.to_string();
        let mut cache = cache.borrow_mut();
        cache
            .add_instrument(updown_binary_option(
                &up_instrument_id,
                "test-market-up",
                "test-market",
                "Up",
                1_000,
                1_300,
            ))
            .expect("test cache should accept active up instrument");
        cache
            .add_instrument(updown_binary_option(
                &down_instrument_id,
                "test-market-down",
                "test-market",
                "Down",
                1_000,
                1_300,
            ))
            .expect("test cache should accept active down instrument");
    }

    fn test_strategy_with_fee_provider(
        fee_provider: Arc<dyn FeeProvider>,
    ) -> BinaryOracleEdgeTaker {
        test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            fee_provider,
            Arc::new(RecordingDecisionEvidenceWriter),
            Arc::new(
                crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                    RecordingDecisionEvidenceWriter,
                )),
            ),
        )
    }

    fn test_strategy_with_fee_provider_and_decision_evidence(
        fee_provider: Arc<dyn FeeProvider>,
        decision_evidence: Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
    ) -> BinaryOracleEdgeTaker {
        test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            fee_provider,
            decision_evidence,
            Arc::new(
                crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                    RecordingDecisionEvidenceWriter,
                )),
            ),
        )
    }

    fn test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
        fee_provider: Arc<dyn FeeProvider>,
        decision_evidence: Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
        submit_admission: Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState>,
    ) -> BinaryOracleEdgeTaker {
        BinaryOracleEdgeTaker::new(
            BinaryOracleEdgeTakerConfig {
                strategy_id: "BINARYORACLEEDGETAKER-001".to_string(),
                order_id_tag: "001".to_string(),
                oms_type: "netting".to_string(),
                client_id: "POLYMARKET".to_string(),
                configured_target_id: "btc_updown_5m".to_string(),
                target_kind: "rotating_market".to_string(),
                rotating_market_family: "updown".to_string(),
                underlying_asset: "BTC".to_string(),
                cadence_seconds: 300,
                cadence_slug_token: "5m".to_string(),
                market_selection_rule: "active_or_next".to_string(),
                retry_interval_seconds: 5,
                blocked_after_seconds: 60,
                reference_venue: "binance_reference".to_string(),
                reference_instrument_id: "BTCUSDT.BINANCE".to_string(),
                use_uuid_client_order_ids: true,
                use_hyphens_in_client_order_ids: false,
                external_order_claims: vec!["ETHUSDT.BINANCE".to_string()],
                manage_contingent_orders: true,
                manage_gtd_expiry: true,
                manage_stop: true,
                market_exit_interval_ms: 250,
                market_exit_max_attempts: 7,
                market_exit_time_in_force: "ioc".to_string(),
                market_exit_reduce_only: false,
                log_events: false,
                log_commands: false,
                log_rejected_due_post_only_as_warning: false,
                entry_order: BinaryOracleEdgeTakerOrderConfig {
                    side: "buy".to_string(),
                    position_side: "long".to_string(),
                    order_type: "limit".to_string(),
                    time_in_force: "fok".to_string(),
                    is_post_only: false,
                    is_reduce_only: false,
                    is_quote_quantity: false,
                },
                exit_order: BinaryOracleEdgeTakerOrderConfig {
                    side: "sell".to_string(),
                    position_side: "long".to_string(),
                    order_type: "market".to_string(),
                    time_in_force: "ioc".to_string(),
                    is_post_only: false,
                    is_reduce_only: false,
                    is_quote_quantity: false,
                },
                warmup_tick_count: 20,
                reentry_cooldown_secs: 30,
                order_notional_target: 1000.0,
                maximum_position_notional: 1000.0,
                book_impact_cap_bps: 15,
                risk_lambda: 0.5,
                edge_threshold_basis_points: -20,
                exit_hysteresis_bps: 5,
                vol_window_secs: 60,
                vol_gap_reset_secs: 10,
                vol_min_observations: 20,
                vol_bridge_valid_secs: 10,
                pricing_kurtosis: 0.0,
                theta_decay_factor: 0.0,
                forced_flat_stale_reference_ms: 1500,
                forced_flat_thin_book_min_liquidity: 100.0,
                lead_agreement_min_corr: 0.8,
                lead_jitter_max_ms: 250,
            },
            StrategyBuildContext::new(fee_provider, decision_evidence, submit_admission),
        )
    }

    #[test]
    fn strategy_core_uses_configured_nt_order_tag_and_oms_type() {
        let strategy = test_strategy();

        assert_eq!(strategy.core.config.order_id_tag.as_deref(), Some("001"));
        assert_eq!(strategy.core.config.oms_type, Some(NtOmsType::Netting));
    }

    #[test]
    fn strategy_core_uses_explicit_configured_nt_strategy_fields() {
        let raw = valid_raw_config();
        let mut errors = Vec::new();
        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
        assert!(errors.is_empty(), "{errors:#?}");

        let config = BinaryOracleEdgeTakerBuilder::parse_config(&raw).unwrap();
        let strategy = BinaryOracleEdgeTaker::new(
            config,
            StrategyBuildContext::new(
                RecordingFeeProvider::cold(),
                Arc::new(RecordingDecisionEvidenceWriter),
                Arc::new(
                    crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(
                        Arc::new(RecordingDecisionEvidenceWriter),
                    ),
                ),
            ),
        );

        assert!(strategy.core.config.use_uuid_client_order_ids);
        assert!(!strategy.core.config.use_hyphens_in_client_order_ids);
        assert_eq!(
            strategy.core.config.external_order_claims,
            Some(vec![InstrumentId::from("ETHUSDT.BINANCE")])
        );
        assert!(strategy.core.config.manage_contingent_orders);
        assert!(strategy.core.config.manage_gtd_expiry);
        assert!(strategy.core.config.manage_stop);
        assert_eq!(strategy.core.config.market_exit_interval_ms, 250);
        assert_eq!(strategy.core.config.market_exit_max_attempts, 7);
        assert_eq!(
            strategy.core.config.market_exit_time_in_force,
            TimeInForce::Ioc
        );
        assert!(!strategy.core.config.market_exit_reduce_only);
        assert!(!strategy.core.config.log_events);
        assert!(!strategy.core.config.log_commands);
        assert!(!strategy.core.config.log_rejected_due_post_only_as_warning);
    }

    fn quote_tick(instrument_id: &str, bid: f64, ask: f64, ts_ms: u64) -> QuoteTick {
        QuoteTick::new_checked(
            InstrumentId::from(instrument_id),
            Price::new(bid, 2),
            Price::new(ask, 2),
            Quantity::new(1.0, 0),
            Quantity::new(1.0, 0),
            nautilus_core::UnixNanos::from(ts_ms.saturating_mul(NANOS_PER_MILLI_U64)),
            nautilus_core::UnixNanos::from(ts_ms.saturating_mul(NANOS_PER_MILLI_U64)),
        )
        .expect("test quote tick should be valid")
    }

    #[test]
    fn reference_quote_tick_updates_pricing_from_configured_reference_data() {
        let mut strategy = test_strategy();

        strategy
            .on_quote(&quote_tick("BTCUSDT.BINANCE", 100.0, 102.0, 1_200))
            .expect("reference quote should process");

        assert_eq!(strategy.pricing.last_reference_fair_value, Some(101.0));
        assert_eq!(
            strategy.pricing.fast_spot,
            Some(fast_spot("binance_reference", 101.0, 1_200))
        );
    }

    #[test]
    fn non_reference_quote_tick_does_not_update_pricing() {
        let mut strategy = test_strategy();

        strategy
            .on_quote(&quote_tick("ETHUSDT.BINANCE", 100.0, 102.0, 1_200))
            .expect("non-reference quote should be ignored");

        assert_eq!(strategy.pricing.last_reference_fair_value, None);
        assert_eq!(strategy.pricing.fast_spot, None);
    }

    fn live_canary_gate_report(
        max_live_order_count: u32,
        max_notional_per_order: Decimal,
    ) -> crate::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport {
        crate::bolt_v3_live_canary_gate::BoltV3LiveCanaryGateReport::for_test(
            max_live_order_count,
            max_notional_per_order,
        )
    }

    #[test]
    fn decision_evidence_failure_rejects_before_nt_submit() {
        let mut strategy = test_strategy_with_fee_provider_and_decision_evidence(
            RecordingFeeProvider::cold(),
            Arc::new(FailingDecisionEvidenceWriter),
        );
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-001-1");
        let order = nautilus_model::orders::OrderAny::Limit(
            nautilus_model::orders::LimitOrder::new_checked(
                nautilus_model::identifiers::TraderId::from("TRADER-001"),
                StrategyId::from(strategy.config.strategy_id.as_str()),
                instrument_id,
                client_order_id,
                OrderSide::Buy,
                quantity,
                price,
                TimeInForce::Fok,
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
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
            )
            .expect("limit order should be valid"),
        );
        let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence {
            strategy_id: strategy.config.strategy_id.clone(),
            intent_kind: crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
            instrument_id: instrument_id.to_string(),
            client_order_id: client_order_id.to_string(),
            order_side: "Buy".to_string(),
            price: price.to_string(),
            quantity: quantity.to_string(),
        };

        let error = strategy
            .submit_order_with_decision_evidence(intent, order, ClientId::from("POLYMARKET"))
            .expect_err("evidence failure must reject before NT submit");

        assert!(
            error.to_string().contains("intent write failed"),
            "{error:#}"
        );
    }

    #[test]
    fn unarmed_submit_admission_rejects_after_evidence_before_nt_submit() {
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        );
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::cold(),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-001-1");
        let order = nautilus_model::orders::OrderAny::Limit(
            nautilus_model::orders::LimitOrder::new_checked(
                nautilus_model::identifiers::TraderId::from("TRADER-001"),
                StrategyId::from(strategy.config.strategy_id.as_str()),
                instrument_id,
                client_order_id,
                OrderSide::Buy,
                quantity,
                price,
                TimeInForce::Fok,
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
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
            )
            .expect("limit order should be valid"),
        );
        let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence {
            strategy_id: strategy.config.strategy_id.clone(),
            intent_kind: crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
            instrument_id: instrument_id.to_string(),
            client_order_id: client_order_id.to_string(),
            order_side: "Buy".to_string(),
            price: price.to_string(),
            quantity: quantity.to_string(),
        };

        let error = strategy
            .submit_order_with_decision_evidence(intent, order, ClientId::from("POLYMARKET"))
            .expect_err("unarmed submit admission must reject before NT submit");

        assert!(
            error.to_string().contains("submit admission is not armed"),
            "{error:#}"
        );
        assert_eq!(submit_admission.admitted_order_count(), 0);
    }

    #[test]
    fn armed_submit_admission_allows_nt_submit_after_evidence() {
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        );
        submit_admission
            .arm(live_canary_gate_report(1, Decimal::new(1, 0)))
            .expect("valid gate report should arm submit admission");
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::cold(),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-001-1");
        let order = nautilus_model::orders::OrderAny::Limit(
            nautilus_model::orders::LimitOrder::new_checked(
                nautilus_model::identifiers::TraderId::from("TRADER-001"),
                StrategyId::from(strategy.config.strategy_id.as_str()),
                instrument_id,
                client_order_id,
                OrderSide::Buy,
                quantity,
                price,
                TimeInForce::Fok,
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
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
            )
            .expect("limit order should be valid"),
        );
        let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence {
            strategy_id: strategy.config.strategy_id.clone(),
            intent_kind: crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
            instrument_id: instrument_id.to_string(),
            client_order_id: client_order_id.to_string(),
            order_side: "Buy".to_string(),
            price: price.to_string(),
            quantity: quantity.to_string(),
        };

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            strategy.submit_order_with_decision_evidence(
                intent,
                order,
                ClientId::from("POLYMARKET"),
            )
        }));

        assert!(
            result.is_err(),
            "test strategy is intentionally not registered with NT; reaching NT submit should panic"
        );
        assert_eq!(submit_admission.admitted_order_count(), 1);
    }

    #[test]
    fn over_notional_submit_admission_rejects_before_nt_submit() {
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        );
        submit_admission
            .arm(live_canary_gate_report(1, Decimal::new(25, 2)))
            .expect("valid gate report should arm submit admission");
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::cold(),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-001-1");
        let order = nautilus_model::orders::OrderAny::Limit(
            nautilus_model::orders::LimitOrder::new_checked(
                nautilus_model::identifiers::TraderId::from("TRADER-001"),
                StrategyId::from(strategy.config.strategy_id.as_str()),
                instrument_id,
                client_order_id,
                OrderSide::Buy,
                quantity,
                price,
                TimeInForce::Fok,
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
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
            )
            .expect("limit order should be valid"),
        );
        let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence {
            strategy_id: strategy.config.strategy_id.clone(),
            intent_kind: crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
            instrument_id: instrument_id.to_string(),
            client_order_id: client_order_id.to_string(),
            order_side: "Buy".to_string(),
            price: price.to_string(),
            quantity: quantity.to_string(),
        };

        let error = strategy
            .submit_order_with_decision_evidence(intent, order, ClientId::from("POLYMARKET"))
            .expect_err("over-cap notional must reject before NT submit");

        assert!(
            error.to_string().contains("notional cap is exceeded"),
            "{error:#}"
        );
        assert_eq!(submit_admission.admitted_order_count(), 0);
    }

    #[test]
    fn exhausted_count_submit_admission_rejects_before_nt_submit() {
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        );
        submit_admission
            .arm(live_canary_gate_report(1, Decimal::new(1, 0)))
            .expect("valid gate report should arm submit admission");
        submit_admission
            .admit(
                &crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionRequest {
                    strategy_id: "strategy-a".to_string(),
                    client_order_id: "client-order-0".to_string(),
                    instrument_id: "instrument-0".to_string(),
                    notional: Decimal::new(50, 2),
                },
            )
            .expect("first admission should consume the only slot");
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::cold(),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-001-1");
        let order = nautilus_model::orders::OrderAny::Limit(
            nautilus_model::orders::LimitOrder::new_checked(
                nautilus_model::identifiers::TraderId::from("TRADER-001"),
                StrategyId::from(strategy.config.strategy_id.as_str()),
                instrument_id,
                client_order_id,
                OrderSide::Buy,
                quantity,
                price,
                TimeInForce::Fok,
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
                nautilus_core::UUID4::new(),
                nautilus_core::UnixNanos::from(1_u64),
            )
            .expect("limit order should be valid"),
        );
        let intent = crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence {
            strategy_id: strategy.config.strategy_id.clone(),
            intent_kind: crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
            instrument_id: instrument_id.to_string(),
            client_order_id: client_order_id.to_string(),
            order_side: "Buy".to_string(),
            price: price.to_string(),
            quantity: quantity.to_string(),
        };

        let error = strategy
            .submit_order_with_decision_evidence(intent, order, ClientId::from("POLYMARKET"))
            .expect_err("exhausted count cap must reject before NT submit");

        assert!(
            error.to_string().contains("order count cap is exhausted"),
            "{error:#}"
        );
        assert_eq!(submit_admission.admitted_order_count(), 1);
    }

    fn ready_to_trade_strategy() -> BinaryOracleEdgeTaker {
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
        strategy.pricing.last_lead_gap_probability = Some(0.0);
        strategy.pricing.last_jitter_penalty_probability = Some(0.0);
        strategy
    }

    fn ready_to_trade_strategy_with_live_fees(
        up_fee_bps: Decimal,
        down_fee_bps: Decimal,
    ) -> BinaryOracleEdgeTaker {
        ready_to_trade_strategy_with_recording_fees(up_fee_bps, down_fee_bps).0
    }

    fn ready_to_trade_strategy_with_recording_fees(
        up_fee_bps: Decimal,
        down_fee_bps: Decimal,
    ) -> (BinaryOracleEdgeTaker, Arc<RecordingFeeProvider>) {
        let fee_provider = RecordingFeeProvider::cold();
        fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", up_fee_bps);
        fee_provider.set_fee("condition-MKT-1-MKT-1-DOWN.POLYMARKET", down_fee_bps);

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
        strategy.pricing.last_lead_gap_probability = Some(0.0);
        strategy.pricing.last_jitter_penalty_probability = Some(0.0);
        (strategy, fee_provider)
    }

    fn pending_entry_state(
        strategy: &BinaryOracleEdgeTaker,
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

    fn set_pending_entry(strategy: &mut BinaryOracleEdgeTaker, pending: PendingEntryState) {
        strategy.exposure = ExposureState::PendingEntry(pending);
    }

    fn set_entry_reconcile_pending(
        strategy: &mut BinaryOracleEdgeTaker,
        pending: PendingEntryState,
        reason: EntryReconcileReason,
    ) {
        strategy.exposure = ExposureState::EntryReconcilePending { pending, reason };
    }

    fn set_managed_position(
        strategy: &mut BinaryOracleEdgeTaker,
        position: OpenPositionState,
        origin: ManagedPositionOrigin,
    ) {
        strategy.exposure = ExposureState::Managed(ManagedPositionState { position, origin });
    }

    fn set_exit_pending(
        strategy: &mut BinaryOracleEdgeTaker,
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

    fn set_blind_recovery(strategy: &mut BinaryOracleEdgeTaker, reason: BlindRecoveryReason) {
        strategy.exposure = ExposureState::BlindRecovery(BlindRecoveryState { reason });
    }

    fn set_unsupported_observed(
        strategy: &mut BinaryOracleEdgeTaker,
        observed: OpenPositionState,
        reason: UnsupportedObservedReason,
    ) {
        strategy.exposure =
            ExposureState::UnsupportedObserved(UnsupportedObservedState { observed, reason });
    }

    fn managed_position_ref(strategy: &BinaryOracleEdgeTaker) -> Option<&OpenPositionState> {
        strategy.managed_position().map(|managed| &managed.position)
    }

    fn pending_exit_ref(strategy: &BinaryOracleEdgeTaker) -> Option<&PendingExitState> {
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
            ruleset_id: "BINARYORACLEEDGETAKER".to_string(),
            decision: SelectionDecision {
                ruleset_id: "BINARYORACLEEDGETAKER".to_string(),
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
        let up_instrument_id = format!("{condition_id}-{up_token_id}.POLYMARKET");
        let down_instrument_id = format!("{condition_id}-{down_token_id}.POLYMARKET");
        CandidateMarket {
            market_id: market_id.to_string(),
            instrument_id: up_instrument_id.clone(),
            up: CandidateOutcome {
                instrument_id: up_instrument_id,
            },
            down: CandidateOutcome {
                instrument_id: down_instrument_id,
            },
            price_to_beat: None,
            start_ts_ms: interval_start_ms,
            seconds_to_end: 300,
        }
    }

    fn updown_binary_option(
        instrument_id: &str,
        market_slug: &str,
        market_id: &str,
        outcome: &str,
        activation_ms: u64,
        expiration_ms: u64,
    ) -> InstrumentAny {
        let mut info = Params::new();
        info.insert(
            "market_slug".to_string(),
            serde_json::Value::String(market_slug.to_string()),
        );
        info.insert(
            "market_id".to_string(),
            serde_json::Value::String(market_id.to_string()),
        );
        InstrumentAny::BinaryOption(BinaryOption::new(
            InstrumentId::from(instrument_id),
            Symbol::from(instrument_id.split('.').next().unwrap_or(instrument_id)),
            AssetClass::Alternative,
            Currency::USDC(),
            (activation_ms.saturating_mul(NANOS_PER_MILLI_U64)).into(),
            (expiration_ms.saturating_mul(NANOS_PER_MILLI_U64)).into(),
            3,
            2,
            Price::from("0.001"),
            Quantity::from("0.01"),
            Some(ustr::Ustr::from(outcome)),
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
            Some(info),
            1.into(),
            1.into(),
        ))
    }

    fn reference_tick(timestamp_ms: u64, price: f64) -> ReferenceSnapshot {
        ReferenceSnapshot {
            ts_ms: timestamp_ms,
            topic: "platform.reference.test.spot".to_string(),
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
            strategy_id: StrategyId::from("BINARYORACLEEDGETAKER-001"),
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
            StrategyId::from("BINARYORACLEEDGETAKER-001"),
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
            StrategyId::from("BINARYORACLEEDGETAKER-001"),
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
            StrategyId::from("BINARYORACLEEDGETAKER-001"),
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
            StrategyId::from("BINARYORACLEEDGETAKER-001"),
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
            strategy_id: StrategyId::from("BINARYORACLEEDGETAKER-001"),
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
    fn production_registry_registers_binary_oracle_edge_taker_kind() {
        let registry = production_strategy_registry().expect("registry should build");
        assert!(registry.get("binary_oracle_edge_taker").is_some());
    }

    #[test]
    fn builder_requires_strategy_id_and_client_id() {
        let raw = toml::toml! {
            warmup_tick_count = 20
            reentry_cooldown_secs = 30
            order_notional_target = 1000.0
            maximum_position_notional = 1000.0
            book_impact_cap_bps = 15
            risk_lambda = 0.5
            edge_threshold_basis_points = -20
            exit_hysteresis_bps = 5
            forced_flat_stale_reference_ms = 1500
            forced_flat_thin_book_min_liquidity = 100.0
            lead_agreement_min_corr = 0.8
            lead_jitter_max_ms = 250
        }
        .into();
        let mut errors = Vec::new();

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

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

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        let error = find_error(&errors, "strategies[0].config.stray_flag", "unknown_field");
        assert!(error.message.contains("unknown field `stray_flag`"));
    }

    #[test]
    fn builder_rejects_non_table_config() {
        let raw = Value::String("not-a-table".to_string());
        let mut errors = Vec::new();

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

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

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

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
        let mut raw = valid_raw_config();
        let raw_table = raw.as_table_mut().expect("valid config must be a table");
        for (field, value) in [
            ("order_notional_target", 1_000),
            ("maximum_position_notional", 1_000),
            ("book_impact_cap_bps", 15),
            ("risk_lambda", 1),
            ("edge_threshold_basis_points", -20),
            ("exit_hysteresis_bps", 5),
            ("pricing_kurtosis", 0),
            ("theta_decay_factor", 0),
            ("forced_flat_thin_book_min_liquidity", 100),
            ("lead_agreement_min_corr", 1),
        ] {
            raw_table.insert(field.to_string(), Value::Integer(value));
        }
        let mut errors = Vec::new();

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            errors.is_empty(),
            "expected integer literals for f64 fields to validate, got: {errors:?}"
        );
    }

    #[test]
    fn builder_accepts_nested_order_shape_without_flat_order_projection() {
        let raw = valid_raw_config();
        let mut errors = Vec::new();

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            errors.is_empty(),
            "nested order shape should validate without flat entry_/exit_ projection: {errors:?}"
        );
    }

    #[test]
    fn builder_requires_pricing_model_fields() {
        let raw = toml::toml! {
            strategy_id = "BINARYORACLEEDGETAKER-001"
            client_id = "POLYMARKET"
            warmup_tick_count = 20
            reentry_cooldown_secs = 30
            order_notional_target = 1000.0
            maximum_position_notional = 1000.0
            book_impact_cap_bps = 15
            risk_lambda = 0.5
            edge_threshold_basis_points = -20
            exit_hysteresis_bps = 5
            forced_flat_stale_reference_ms = 1500
            forced_flat_thin_book_min_liquidity = 100.0
            lead_agreement_min_corr = 0.8
            lead_jitter_max_ms = 250

            [entry_order]
            side = "buy"
            position_side = "long"
            order_type = "limit"
            time_in_force = "fok"
            is_post_only = false
            is_reduce_only = false
            is_quote_quantity = false

            [exit_order]
            side = "sell"
            position_side = "long"
            order_type = "market"
            time_in_force = "ioc"
            is_post_only = false
            is_reduce_only = false
            is_quote_quantity = false
        }
        .into();
        let mut errors = Vec::new();

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.cadence_seconds")
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
    fn pricing_state_requires_fast_spot_for_pricing_and_keeps_reference_separate() {
        let config = test_strategy().config.clone();
        let mut pricing = PricingState::from_config(&config);

        pricing.observe_reference_snapshot(
            &reference_tick(1_000, 3_100.0),
            config.lead_agreement_min_corr,
            config.lead_jitter_max_ms,
        );
        assert_eq!(pricing.spot_price(), None);
        assert_eq!(pricing.last_reference_fair_value, Some(3_100.0));

        let snapshot = ReferenceSnapshot {
            ts_ms: 1_100,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_101.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_101.0, 1_100),
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
    fn pricing_state_requires_reference_anchor_for_fast_spot_selection() {
        let config = test_strategy().config.clone();
        let mut pricing = PricingState::from_config(&config);

        pricing.observe_reference_snapshot(
            &ReferenceSnapshot {
                ts_ms: 1_000,
                topic: "platform.reference.test.spot".to_string(),
                fair_value: None,
                confidence: 1.0,
                venues: vec![orderbook_venue("bybit", 0.9, 3_102.0, 1_000)],
            },
            config.lead_agreement_min_corr,
            config.lead_jitter_max_ms,
        );

        assert_eq!(pricing.spot_price(), None);
        assert_eq!(pricing.last_lead_gap_probability, None);
        assert_eq!(pricing.last_jitter_penalty_probability, None);
        assert_eq!(pricing.last_lead_agreement_corr, None);
    }

    #[test]
    fn pricing_state_applies_lead_quality_thresholds() {
        let mut config = test_strategy().config.clone();
        config.lead_agreement_min_corr = 0.9999;
        let mut pricing = PricingState::from_config(&config);

        let snapshot = ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_100.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_100.0, 1_000),
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
        assert_eq!(pricing.spot_price(), None);
        assert_eq!(pricing.last_reference_fair_value, Some(3_100.0));
    }

    #[test]
    fn pricing_state_clears_fast_spot_when_no_fast_venue_remains() {
        let config = test_strategy().config.clone();
        let mut pricing = PricingState::from_config(&config);

        pricing.observe_reference_snapshot(
            &ReferenceSnapshot {
                ts_ms: 1_000,
                topic: "platform.reference.test.spot".to_string(),
                fair_value: Some(3_100.0),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("reference", 1.0, 3_100.0, 1_000),
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
                topic: "platform.reference.test.spot".to_string(),
                fair_value: Some(3_101.0),
                confidence: 1.0,
                venues: vec![oracle_venue("reference", 1.0, 3_101.0, 1_100)],
            },
            config.lead_agreement_min_corr,
            config.lead_jitter_max_ms,
        );

        assert!(pricing.fast_spot.is_none());
        assert_eq!(pricing.spot_price(), None);
        assert_eq!(pricing.last_reference_fair_value, Some(3_101.0));
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
                topic: "platform.reference.test.spot".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("reference", 1.0, fair_value, ts_ms),
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
                topic: "platform.reference.test.spot".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("reference", 1.0, fair_value, ts_ms),
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
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_104.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_104.0, 5_000),
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
                topic: "platform.reference.test.spot".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("reference", 1.0, fair_value, ts_ms),
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
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_103.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_103.0, 4_000),
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
                topic: "platform.reference.test.spot".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("reference", 1.0, fair_value, ts_ms),
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
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_102.2),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_102.2, 3_100),
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
                topic: "platform.reference.test.spot".to_string(),
                fair_value: Some(fair_value),
                confidence: 1.0,
                venues: vec![
                    oracle_venue("reference", 1.0, fair_value, ts_ms),
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
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_102.5),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_102.5, 4_201),
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
    fn entry_evaluation_log_fields_fail_closed_without_fast_spot() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.pricing.fast_spot = None;
        strategy.pricing.last_reference_fair_value = Some(3_101.0);
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
        strategy.pricing.realized_vol_source_venue = Some("bybit".to_string());

        let submission = strategy.entry_submission_decision_at(1_200);
        let fields = strategy.entry_evaluation_log_fields_at(1_200, &submission);

        assert_eq!(fields.spot_venue_name, None);
        assert_eq!(fields.spot_price, None);
        assert_eq!(
            fields.pricing_blocked_by,
            vec![EntryPricingBlockReason::SpotPriceMissing]
        );
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
    fn fair_probability_helper_fails_closed_when_expired() {
        assert!(
            compute_fair_probability_up(&FairProbabilityInputs {
                spot_price: 3_105.0,
                strike_price: 3_100.0,
                seconds_to_expiry: 0,
                realized_vol: 0.45,
                pricing_kurtosis: 0.0,
            })
            .is_none(),
            "expired markets must not produce a step-function entry probability"
        );
    }

    #[test]
    fn theta_scaler_helper_increases_near_expiry_and_can_be_disabled() {
        let start = compute_theta_scaler(&ThetaScalerInputs {
            seconds_to_expiry: 300,
            cadence_seconds: 300,
            theta_decay_factor: 1.5,
        })
        .expect("valid theta inputs should compute");
        let near_expiry = compute_theta_scaler(&ThetaScalerInputs {
            seconds_to_expiry: 30,
            cadence_seconds: 300,
            theta_decay_factor: 1.5,
        })
        .expect("valid theta inputs should compute");

        assert!((start - 1.0).abs() < 1e-9);
        assert!(near_expiry > start);
        assert_eq!(
            compute_theta_scaler(&ThetaScalerInputs {
                seconds_to_expiry: 30,
                cadence_seconds: 300,
                theta_decay_factor: 0.0,
            }),
            Some(1.0)
        );
        assert!(
            compute_theta_scaler(&ThetaScalerInputs {
                seconds_to_expiry: 30,
                cadence_seconds: 0,
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
                topic: "platform.reference.test.spot".to_string(),
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
        strategy.config.edge_threshold_basis_points = 10;
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
            strategy.managed_position().map(|managed| managed.origin),
            Some(ManagedPositionOrigin::RecoveryBootstrap)
        );
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
    fn position_change_preserves_pending_exit_correlation() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let position_id = PositionId::from("P-EXIT-CHANGE");
        let exit_client_order_id = ClientOrderId::from("EXIT-CHANGE");
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

        strategy.materialize_position_from_event(
            instrument_id,
            position_id,
            OrderSide::Buy,
            PositionSide::Long,
            Quantity::new(7.0, 2),
            0.470,
        );

        let exit_pending = strategy
            .exposure
            .exit_pending()
            .expect("position change should keep exit pending");
        assert_eq!(
            exit_pending.pending_exit.client_order_id,
            exit_client_order_id
        );
        assert_eq!(exit_pending.pending_exit.position_id, Some(position_id));
        assert!(!exit_pending.pending_exit.fill_received);
        assert!(!exit_pending.pending_exit.close_received);

        let position = exit_pending
            .position
            .as_ref()
            .expect("exit pending should keep managed position");
        assert_eq!(position.origin, ManagedPositionOrigin::StrategyEntry);
        assert_eq!(position.position.quantity, Quantity::new(7.0, 2));
        assert_eq!(position.position.avg_px_open, 0.470);
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
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
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
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
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
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
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
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
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
    fn down_entry_submission_price_uses_configured_order_side_price() {
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

        strategy.config.entry_order.side = "sell".to_string();
        strategy.config.entry_order.position_side = "short".to_string();
        strategy.config.exit_order.side = "buy".to_string();
        strategy.config.exit_order.position_side = "short".to_string();

        assert_eq!(
            strategy.submission_entry_price(OutcomeSide::Down),
            Some(0.40)
        );
        assert_eq!(
            strategy.executable_entry_cost(OutcomeSide::Down),
            Some(0.40)
        );
    }

    #[test]
    fn entry_book_impact_cap_uses_configured_sell_side_book() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.config.entry_order.side = "sell".to_string();
        strategy.config.entry_order.position_side = "short".to_string();
        strategy.config.exit_order.side = "buy".to_string();
        strategy.config.exit_order.position_side = "short".to_string();
        strategy.config.book_impact_cap_bps = 0;
        strategy.active.books.down.bid_levels.clear();
        strategy.active.books.down.ask_levels.clear();
        strategy
            .active
            .books
            .down
            .bid_levels
            .insert(Price::new(0.44, 2), 7.0);
        strategy
            .active
            .books
            .down
            .bid_levels
            .insert(Price::new(0.42, 2), 100.0);
        strategy
            .active
            .books
            .down
            .ask_levels
            .insert(Price::new(0.60, 2), 100.0);
        strategy.active.books.down.best_bid = Some(0.44);
        strategy.active.books.down.best_ask = Some(0.60);

        assert_eq!(
            strategy.visible_book_notional_cap(OutcomeSide::Down),
            Some(3.08)
        );
    }

    #[test]
    fn configured_short_position_contract_is_supported_when_entry_exit_contract_is_coherent() {
        let contract = ConfiguredPositionContract {
            entry_order_side: OrderSide::Sell,
            entry_position_side: PositionSide::Short,
            exit_order_side: OrderSide::Buy,
            exit_position_side: PositionSide::Short,
        };

        assert!(supports_strategy_managed_position(
            OrderSide::Sell,
            PositionSide::Short,
            contract
        ));
    }

    #[test]
    fn position_event_without_context_does_not_guess_side_from_suffix() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = InstrumentId::from("external-MKT-1-UP.POLYMARKET");

        strategy.on_position_opened(position_opened_event(
            instrument_id,
            PositionId::from("P-SUFFIX-001"),
            Quantity::new(10.0, 2),
            0.450,
        ));

        assert_eq!(
            managed_position_ref(&strategy).and_then(|position| position.outcome_side),
            None
        );
        let position = managed_position_ref(&strategy).expect("position should be tracked");
        assert_eq!(position.market_id, None);
        assert_eq!(position.outcome_fees, OutcomeFeeState::empty());
        assert_eq!(position.interval_open, None);
        assert_eq!(position.selection_published_at_ms, None);
        assert_eq!(position.seconds_to_expiry_at_selection, None);
        assert_eq!(
            strategy.managed_position().map(|managed| managed.origin),
            Some(ManagedPositionOrigin::RecoveryBootstrap)
        );
    }

    #[test]
    fn production_outcome_side_inference_does_not_parse_instrument_suffixes() {
        let source = include_str!("binary_oracle_edge_taker.rs");
        let production = source
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .expect("production source should precede cfg(test) test module");
        let up_suffix = format!("{}{}{}", "-", "UP", ".");
        let down_suffix = format!("{}{}{}", "-", "DOWN", ".");

        assert!(
            !production.contains(&up_suffix),
            "production strategy code must not infer outcome side from instrument-id text suffix"
        );
        assert!(
            !production.contains(&down_suffix),
            "production strategy code must not infer outcome side from instrument-id text suffix"
        );
    }

    #[test]
    fn book_impact_cap_is_derived_from_vwap_slippage_against_best_touch() {
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");
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
            .max_execution_within_vwap_slippage_bps(OrderSide::Buy, 0)
            .expect("best-touch-only size should exist");
        let one_hundred_bps = state
            .max_execution_within_vwap_slippage_bps(OrderSide::Buy, 100)
            .expect("partial next-level size should exist");
        let loose = state
            .max_execution_within_vwap_slippage_bps(OrderSide::Buy, 5_000)
            .expect("full displayed size should exist");

        assert_eq!(zero_bps.quantity, 10.0);
        assert!(one_hundred_bps.quantity > zero_bps.quantity);
        assert!(one_hundred_bps.quantity < loose.quantity);
        assert_eq!(loose.quantity, 20.0);
        assert!(one_hundred_bps.vwap_price > zero_bps.vwap_price);
    }

    #[test]
    fn book_impact_cap_config_changes_sizing_decision() {
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");

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

        let loose_cap = loose.visible_book_notional_cap(OutcomeSide::Up);
        let tight_cap = tight.visible_book_notional_cap(OutcomeSide::Up);

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
            managed_position_ref(&strategy)
                .and_then(|p| p.outcome_fees.up_instrument_id)
                .map(|instrument_id| instrument_id.to_string())
                .as_deref(),
            Some("condition-MKT-1-MKT-1-UP.POLYMARKET")
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
        assert_eq!(
            strategy.managed_position().map(|managed| managed.origin),
            Some(ManagedPositionOrigin::StrategyEntry)
        );
        assert!(strategy.pending_entry().is_none());
    }

    #[test]
    fn late_entry_terminal_events_preserve_entry_reconcile_fail_closed_state() {
        let entry_client_order_id = ClientOrderId::from("ENTRY-LATE-TERM");
        let instrument_id = InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET");

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
    fn book_delta_submit_admission_error_does_not_escape_actor_loop() {
        let mut direct = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut direct);
        let direct_error = direct
            .try_submit_entry_order(1_200)
            .expect_err("test setup must reach unarmed submit admission");
        assert!(
            direct_error
                .to_string()
                .contains("submit admission is not armed"),
            "test setup must prove submit-admission failure path: {direct_error:#}"
        );

        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut strategy);
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let decision = strategy.entry_submission_decision_at(1_200);
        assert!(
            decision.instrument_id.is_some()
                && decision.order_side.is_some()
                && decision.price.is_some()
                && decision.quantity_value.is_some()
                && decision.blocked_reason.is_none(),
            "test setup must reach submit admission path; got {decision:#?}"
        );

        let result = strategy.on_book_deltas(&book_deltas(
            instrument_id,
            &[(BookAction::Update, OrderSide::Sell, 0.44, 500.0)],
        ));

        assert!(
            result.is_ok(),
            "book-delta submit failures must be logged and contained inside the strategy actor: {result:#?}"
        );
        assert!(matches!(strategy.exposure, ExposureState::Flat));
    }

    #[test]
    fn book_delta_exit_submit_admission_error_does_not_escape_actor_loop() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.active.phase = SelectionPhase::Freeze;
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            position_id: PositionId::from("P-EXIT-SUBMIT-ERROR"),
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
        register_test_strategy_with_active_instruments(&mut strategy);
        let decision = strategy.exit_submission_decision_at(1_200);
        assert!(
            decision.instrument_id.is_some()
                && decision.order_side.is_some()
                && decision.price.is_some()
                && decision.quantity.is_some()
                && decision.blocked_reason.is_none(),
            "test setup must reach exit submit admission path; got {decision:#?}"
        );

        let result = strategy.on_book_deltas(&book_deltas(
            instrument_id,
            &[(BookAction::Update, OrderSide::Buy, 0.44, 500.0)],
        ));

        assert!(
            result.is_ok(),
            "book-delta exit submit failures must be logged and contained inside the strategy actor: {result:#?}"
        );
        assert!(matches!(strategy.exposure, ExposureState::Managed(_)));
        assert_eq!(strategy.last_reported_exposure_occupancy.get(), None);
    }

    #[test]
    fn book_delta_entry_reconcile_pending_does_not_try_new_entry() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut strategy);
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let pending = pending_entry_state(
            &strategy,
            ClientOrderId::from("ENTRY-RECONCILE-BOOK-DELTA"),
            instrument_id,
            OutcomeSide::Up,
            strategy.active.books.up.clone(),
        );
        set_entry_reconcile_pending(
            &mut strategy,
            pending,
            EntryReconcileReason::AwaitingPositionMaterialization,
        );

        let result = strategy.on_book_deltas(&book_deltas(
            instrument_id,
            &[(BookAction::Update, OrderSide::Sell, 0.43, 500.0)],
        ));

        assert!(
            result.is_ok(),
            "book-delta handling must not escape while entry reconciliation is pending: {result:#?}"
        );
        assert!(matches!(
            strategy.exposure,
            ExposureState::EntryReconcilePending { .. }
        ));
        assert_eq!(strategy.last_reported_exposure_occupancy.get(), None);
    }

    #[test]
    fn position_closed_releases_entry_reconcile_pending_for_same_instrument() {
        let mut strategy = ready_to_trade_strategy();
        let entry_client_order_id = ClientOrderId::from("ENTRY-CLOSED-BEFORE-OPEN");
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let pending = pending_entry_state(
            &strategy,
            entry_client_order_id,
            instrument_id,
            OutcomeSide::Up,
            strategy.active.books.up.clone(),
        );
        set_entry_reconcile_pending(
            &mut strategy,
            pending,
            EntryReconcileReason::AwaitingPositionMaterialization,
        );

        strategy.on_position_closed(position_closed_event(
            instrument_id,
            PositionId::from("P-CLOSED-BEFORE-OPEN"),
        ));

        assert!(matches!(strategy.exposure, ExposureState::Flat));
        assert!(strategy.pending_entry().is_none());
    }

    #[test]
    fn position_closed_keeps_entry_reconcile_pending_for_different_instrument() {
        let mut strategy = ready_to_trade_strategy();
        let entry_client_order_id = ClientOrderId::from("ENTRY-CLOSE-OTHER-INSTRUMENT");
        let pending_instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let other_instrument_id = strategy.active.books.down.instrument_id.unwrap();
        let pending = pending_entry_state(
            &strategy,
            entry_client_order_id,
            pending_instrument_id,
            OutcomeSide::Up,
            strategy.active.books.up.clone(),
        );
        set_entry_reconcile_pending(
            &mut strategy,
            pending,
            EntryReconcileReason::AwaitingPositionMaterialization,
        );

        strategy.on_position_closed(position_closed_event(
            other_instrument_id,
            PositionId::from("P-CLOSED-OTHER-INSTRUMENT"),
        ));

        assert!(matches!(
            strategy.exposure,
            ExposureState::EntryReconcilePending { .. }
        ));
        assert!(strategy.pending_entry().is_some());
    }

    #[test]
    fn position_closed_releases_unsupported_observed_for_same_position() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let position_id = PositionId::from("P-UNSUPPORTED-CLOSED");
        let book = strategy.active.books.up.clone();
        set_unsupported_observed(
            &mut strategy,
            OpenPositionState {
                market_id: Some("MKT-1".to_string()),
                instrument_id,
                position_id,
                outcome_side: None,
                outcome_fees: OutcomeFeeState::empty(),
                historical_entry_fee_bps: None,
                entry_order_side: OrderSide::Sell,
                side: PositionSide::Short,
                quantity: Quantity::new(5.0, 2),
                avg_px_open: 0.480,
                interval_open: None,
                selection_published_at_ms: None,
                seconds_to_expiry_at_selection: None,
                book,
            },
            UnsupportedObservedReason::BootstrappedUnsupportedContract,
        );

        strategy.on_position_closed(position_closed_event(instrument_id, position_id));

        assert!(matches!(strategy.exposure, ExposureState::Flat));
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
    fn unsupported_entry_fill_without_matching_context_keeps_unknown_side_absent() {
        let mut strategy = ready_to_trade_strategy();
        let pending_instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let fill_instrument_id = strategy.active.books.down.instrument_id.unwrap();
        let entry_client_order_id = ClientOrderId::from("ENTRY-MISMATCHED-FILL");
        let pending = pending_entry_state(
            &strategy,
            entry_client_order_id,
            pending_instrument_id,
            OutcomeSide::Up,
            strategy.active.books.up.clone(),
        );
        set_pending_entry(&mut strategy, pending);

        strategy
            .on_order_filled(&order_filled_event_with_details(
                entry_client_order_id,
                fill_instrument_id,
                Some(PositionId::from("P-MISMATCHED-FILL")),
                OrderSide::Sell,
            ))
            .expect("unsupported mismatched fill should fail closed");

        let ExposureState::BlindRecovery(recovery) = &strategy.exposure else {
            panic!("expected blind recovery, got {:?}", strategy.exposure);
        };
        assert_eq!(
            recovery.reason,
            BlindRecoveryReason::InvalidLivePosition {
                entry_order_side: OrderSide::Sell,
                side: None,
            }
        );
        assert!(strategy.managed_position().is_none());
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
            open_position
                .outcome_fees
                .up_instrument_id
                .map(|instrument_id| instrument_id.to_string())
                .as_deref(),
            Some("condition-MKT-1-MKT-1-UP.POLYMARKET")
        );
        assert_eq!(open_position.book.best_bid, preserved_book.best_bid);
    }

    #[test]
    fn interval_open_captures_first_reference_tick_at_or_after_market_start() {
        let mut strategy = test_strategy();
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

        strategy.observe_reference_snapshot(&reference_tick(900, 3_100.0));
        assert!(strategy.active.interval_open.is_none());

        strategy.observe_reference_snapshot(&reference_tick(1_000, 3_101.0));
        assert_eq!(strategy.active.interval_open, Some(3_101.0));
    }

    #[test]
    fn interval_open_uses_raw_reference_price_not_fused_reference_value() {
        let mut strategy = test_strategy();
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_107.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_100.0, 1_000),
                orderbook_venue("bybit", 0.9, 3_120.0, 1_000),
            ],
        });

        assert_eq!(strategy.active.interval_open, Some(3_100.0));
    }

    #[test]
    fn interval_open_prefers_polymarket_price_to_beat_over_reference() {
        let mut strategy = test_strategy();
        let mut snapshot = active_snapshot_with_start("MKT-1", 1_000);
        let SelectionState::Active { market } = &mut snapshot.decision.state else {
            panic!("expected active snapshot");
        };
        market.price_to_beat = Some(3_099.0);
        strategy.apply_selection_snapshot(snapshot);

        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_107.0),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_100.0, 1_000),
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
            topic: "platform.reference.test.spot".to_string(),
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

        fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", Decimal::new(175, 2));
        strategy.refresh_fee_readiness();
        assert!(strategy.active.outcome_fees.up_ready);
        assert!(!strategy.active.outcome_fees.down_ready);

        fee_provider.set_fee(
            "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
            Decimal::new(180, 2),
        );
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
                "condition-MKT-1-MKT-1-UP.POLYMARKET".to_string(),
                "condition-MKT-1-MKT-1-DOWN.POLYMARKET".to_string(),
                "condition-MKT-2-MKT-2-UP.POLYMARKET".to_string(),
                "condition-MKT-2-MKT-2-DOWN.POLYMARKET".to_string(),
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
                "condition-MKT-1-MKT-1-UP.POLYMARKET".to_string(),
                "condition-MKT-1-MKT-1-DOWN.POLYMARKET".to_string(),
                "condition-MKT-1-MKT-1-UP.POLYMARKET".to_string(),
                "condition-MKT-1-MKT-1-DOWN.POLYMARKET".to_string(),
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
                "condition-MKT-1-MKT-1-UP.POLYMARKET".to_string(),
                "condition-MKT-1-MKT-1-DOWN.POLYMARKET".to_string(),
                "condition-MKT-1-MKT-1-UP.POLYMARKET".to_string(),
                "condition-MKT-1-MKT-1-DOWN.POLYMARKET".to_string(),
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

        fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", Decimal::new(175, 2));
        strategy.refresh_fee_readiness();
        assert!(strategy.active.outcome_fees.up_ready);
        assert!(!strategy.active.outcome_fees.down_ready);

        fee_provider.set_fee(
            "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
            Decimal::new(180, 2),
        );
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
        assert_eq!(
            active
                .outcome_fees
                .up_instrument_id
                .map(|instrument_id| instrument_id.to_string())
                .as_deref(),
            Some("condition-MKT-1-MKT-1-UP.POLYMARKET")
        );
        assert_eq!(
            active
                .outcome_fees
                .down_instrument_id
                .map(|instrument_id| instrument_id.to_string())
                .as_deref(),
            Some("condition-MKT-1-MKT-1-DOWN.POLYMARKET")
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
        fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", Decimal::new(175, 2));
        fee_provider.set_fee(
            "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
            Decimal::new(180, 2),
        );
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
        fee_provider.set_fee("condition-MKT-2-MKT-2-UP.POLYMARKET", Decimal::new(175, 2));
        fee_provider.set_fee(
            "condition-MKT-2-MKT-2-DOWN.POLYMARKET",
            Decimal::new(180, 2),
        );
        let mut strategy = test_strategy_with_fee_provider(fee_provider);

        strategy.apply_selection_snapshot(active_snapshot("MKT-1"));
        strategy.apply_selection_snapshot(active_snapshot("MKT-2"));

        assert!(strategy.active.outcome_fees.up_ready);
        assert!(strategy.active.outcome_fees.down_ready);
    }

    #[test]
    fn same_market_new_interval_with_cached_fee_rates_stays_ready_while_refresh_runs() {
        let fee_provider = RecordingFeeProvider::cold();
        fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", Decimal::new(175, 2));
        fee_provider.set_fee(
            "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
            Decimal::new(180, 2),
        );
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
    fn strategy_selects_configured_updown_target_from_nt_binary_option_metadata() {
        let strategy = test_strategy();
        let current_start = 1_746_000_000_i64;
        let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
            &strategy.config.underlying_asset,
            &strategy.config.cadence_slug_token,
            current_start,
        );
        let instruments = vec![
            updown_binary_option(
                "token-up.POLYMARKET",
                &market_slug,
                "market-1",
                "Up",
                current_start as u64 * MILLIS_PER_SECOND_U64,
                current_start as u64 * MILLIS_PER_SECOND_U64
                    + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64,
            ),
            updown_binary_option(
                "token-down.POLYMARKET",
                &market_slug,
                "market-1",
                "Down",
                current_start as u64 * MILLIS_PER_SECOND_U64,
                current_start as u64 * MILLIS_PER_SECOND_U64
                    + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64,
            ),
        ];

        let snapshot = selection_snapshot_from_instruments(
            &strategy.config,
            &instruments,
            current_start as u64 * MILLIS_PER_SECOND_U64 + 1,
        );

        let SelectionState::Active { market } = snapshot.decision.state else {
            panic!("configured target should select active market: {snapshot:?}");
        };
        assert_eq!(market.market_id, "market-1");
        assert_eq!(market.up.instrument_id, "token-up.POLYMARKET");
        assert_eq!(market.down.instrument_id, "token-down.POLYMARKET");
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
    fn task4_lead_arbitration_uses_reference_when_no_fast_venue_is_eligible() {
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
            expected_ev_per_notional: 2.0,
            risk_lambda: 0.1,
            order_notional_target: 100.0,
            maximum_position_notional: 100.0,
            impact_cap_notional: 100.0,
        });
        let high_risk = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 2.0,
            risk_lambda: 2.0,
            order_notional_target: 100.0,
            maximum_position_notional: 100.0,
            impact_cap_notional: 100.0,
        });
        let capped = choose_robust_size(&RobustSizingInputs {
            expected_ev_per_notional: 2.0,
            risk_lambda: 0.1,
            order_notional_target: 100.0,
            maximum_position_notional: 12.0,
            impact_cap_notional: 7.5,
        });

        assert!(high_risk < low_risk);
        assert_eq!(capped, 7.5);
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_notional: 0.0,
                risk_lambda: 0.1,
                order_notional_target: 100.0,
                maximum_position_notional: 100.0,
                impact_cap_notional: 100.0,
            }),
            0.0
        );
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_notional: 2.0,
                risk_lambda: 0.0,
                order_notional_target: 100.0,
                maximum_position_notional: 100.0,
                impact_cap_notional: 100.0,
            }),
            100.0
        );
        assert_eq!(
            choose_robust_size(&RobustSizingInputs {
                expected_ev_per_notional: 2.0,
                risk_lambda: -0.1,
                order_notional_target: 100.0,
                maximum_position_notional: 100.0,
                impact_cap_notional: 100.0,
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
                EntryBlockReason::ForcedFlat(ForcedFlatReason::ThinBook),
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
    fn task5_entry_order_plan_uses_configured_tif_and_side_specific_best_price() {
        let up = build_entry_order_plan(&EntryOrderPlanInputs {
            client_order_id: ClientOrderId::from("ENTRY-UP"),
            instrument_id: InstrumentId::from("condition-MKT-1-MKT-1-UP.POLYMARKET"),
            order_side: OrderSide::Buy,
            quantity: Quantity::non_zero(5.0, 0),
            price_precision: 2,
            time_in_force: TimeInForce::Fok,
            best_bid: 0.43,
            best_ask: 0.45,
        })
        .expect("up entry should have a valid plan");
        let down = build_entry_order_plan(&EntryOrderPlanInputs {
            client_order_id: ClientOrderId::from("ENTRY-DOWN"),
            instrument_id: InstrumentId::from("condition-MKT-1-MKT-1-DOWN.POLYMARKET"),
            order_side: OrderSide::Sell,
            quantity: Quantity::non_zero(5.0, 0),
            price_precision: 2,
            time_in_force: TimeInForce::Ioc,
            best_bid: 0.43,
            best_ask: 0.45,
        })
        .expect("down entry should have a valid plan");

        assert_eq!(up.order_side, OrderSide::Buy);
        assert_eq!(up.price, Price::new(0.45, 2));
        assert_eq!(up.time_in_force, TimeInForce::Fok);
        assert_eq!(down.order_side, OrderSide::Sell);
        assert_eq!(down.price, Price::new(0.43, 2));
        assert_eq!(down.time_in_force, TimeInForce::Ioc);
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
            last_reference_ts_ms: Some(1_000),
            now_ms: 3_000,
            stale_reference_after_ms: 1_500,
            liquidity_available: Some(50.0),
            min_liquidity_required: 100.0,
            fast_venue_incoherent: true,
        });

        assert_eq!(
            reasons,
            vec![
                ForcedFlatReason::Freeze,
                ForcedFlatReason::StaleReference,
                ForcedFlatReason::ThinBook,
                ForcedFlatReason::MetadataMismatch,
                ForcedFlatReason::FastVenueIncoherent,
            ]
        );
    }

    #[test]
    fn task5_missing_liquidity_is_thin_book() {
        let reasons = evaluate_forced_flat_predicates(&ForcedFlatInputs {
            phase: SelectionPhase::Active,
            metadata_matches_selection: true,
            last_reference_ts_ms: Some(1_000),
            now_ms: 1_250,
            stale_reference_after_ms: 1_500,
            liquidity_available: None,
            min_liquidity_required: 100.0,
            fast_venue_incoherent: false,
        });

        assert_eq!(reasons, vec![ForcedFlatReason::ThinBook]);
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
                EntryBlockReason::ForcedFlat(ForcedFlatReason::StaleReference),
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
                .expected_ev_per_notional
                .is_some_and(|value| value > 0.0)
        );
        assert!(
            decision
                .book_impact_cap_notional
                .is_some_and(|value| value > 0.0)
        );
        assert!(decision.sized_notional.is_some_and(|value| value > 0.0));
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
    fn task6_entry_evaluation_requires_live_uncertainty_components() {
        let mut strategy =
            ready_to_trade_strategy_with_live_fees(Decimal::new(250, 2), Decimal::new(250, 2));
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_100.4, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
        strategy.pricing.last_lead_gap_probability = None;
        strategy.pricing.last_jitter_penalty_probability = None;

        let decision = strategy.entry_evaluation_at(1_200);

        assert_eq!(
            decision.pricing_blocked_by,
            vec![EntryPricingBlockReason::UncertaintyBandUnavailable]
        );
        assert_eq!(decision.uncertainty_band_probability, None);
    }

    #[test]
    fn task6_entry_evaluation_applies_theta_scaled_threshold_at_boundary() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_120.0, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
        strategy.pricing.realized_vol.bridge_valid_ms = 1_000_000;
        strategy.config.edge_threshold_basis_points = 2_000;

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
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_100.5),
            confidence: 1.0,
            venues: vec![
                oracle_venue("reference", 1.0, 3_100.5, 1_200),
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
        assert!(
            fields
                .expected_ev_per_notional
                .is_some_and(|value| value > 0.0)
        );
        assert_eq!(
            fields.maximum_position_notional,
            strategy.config.maximum_position_notional
        );
        assert_eq!(fields.risk_lambda, strategy.config.risk_lambda);
        assert_eq!(
            fields.book_impact_cap_bps,
            strategy.config.book_impact_cap_bps
        );
        assert!(
            fields
                .book_impact_cap_notional
                .is_some_and(|value| value > 0.0)
        );
        assert!(fields.sized_notional.is_some_and(|value| value > 0.0));
        assert!(!fields.final_fee_amount_known);
    }

    #[test]
    fn exit_evaluation_log_fields_use_position_context_after_rotation() {
        let fee_provider = RecordingFeeProvider::cold();
        fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", Decimal::new(100, 2));
        fee_provider.set_fee(
            "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
            Decimal::new(200, 2),
        );
        fee_provider.set_fee("condition-MKT-2-MKT-2-UP.POLYMARKET", Decimal::new(300, 2));
        fee_provider.set_fee(
            "condition-MKT-2-MKT-2-DOWN.POLYMARKET",
            Decimal::new(400, 2),
        );

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
        assert_eq!(fields.spot_price, None);
        assert_eq!(fields.spot_venue_name, None);
        assert_eq!(fields.interval_open, Some(3_100.0));
        assert_eq!(fields.seconds_to_expiry, Some(299));
        assert_eq!(fields.fair_probability_up, None);
        assert_eq!(fields.hold_ev_bps, None);
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

        fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", Decimal::new(300, 2));
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

        fee_provider.set_fee("condition-MKT-1-MKT-1-UP.POLYMARKET", Decimal::new(300, 2));
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
                outcome_fees: OutcomeFeeState::empty(),
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
                outcome_fees: OutcomeFeeState::empty(),
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
    fn task6_exit_decision_requires_live_uncertainty_components() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let open_position = OpenPositionState {
            market_id: Some("MKT-1".to_string()),
            instrument_id: strategy.active.books.up.instrument_id.unwrap(),
            position_id: PositionId::from("P-UP-MISSING-UNCERTAINTY"),
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
        strategy.pricing.last_lead_gap_probability = None;
        strategy.pricing.last_jitter_penalty_probability = None;

        let decision = strategy.exit_submission_decision_at(1_200);

        assert_eq!(decision.evaluation.hold_ev_bps, None);
        assert!(decision.evaluation.exit_ev_bps.is_some());
        assert_eq!(
            decision.evaluation.exit_decision,
            Some(ExitDecision::ExitFailClosed)
        );
        assert_eq!(decision.order_side, Some(OrderSide::Sell));
        assert_eq!(
            decision.instrument_id,
            strategy.active.books.up.instrument_id
        );
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
