use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, VecDeque},
    rc::Rc,
    str::FromStr,
    sync::Arc,
};

use anyhow::{Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
use nautilus_common::{
    actor::DataActor, cache::Cache, clock::TestClock, component::Component, timer::TimeEvent,
};
use nautilus_core::{Params, UnixNanos};
#[cfg(not(test))]
use nautilus_model::enums::BookType;
use nautilus_model::{
    data::{QuoteTick, TradeTick},
    enums::PositionSide,
};
use nautilus_model::{
    enums::{
        AggressorSide, BookAction, OmsType as NtOmsType, OrderSide, OrderType, TimeInForce,
        TrailingOffsetType, TriggerType,
    },
    identifiers::{ClientId, ClientOrderId, InstrumentId, PositionId, StrategyId, TraderId, Venue},
    instruments::{Instrument, InstrumentAny},
    orders::{Order, OrderAny},
    types::{Price, Quantity},
};
use nautilus_portfolio::portfolio::Portfolio;
use nautilus_system::trader::Trader;
use nautilus_trading::{Strategy, StrategyConfig, StrategyCore, nautilus_strategy};
use rust_decimal::{
    Decimal,
    prelude::{FromPrimitive, ToPrimitive},
};
use serde::{Deserialize, Serialize};
use toml::Value;

use crate::{
    bolt_v3_config::{PRICE_GATE_VALUE_KIND, RESOLUTION_GATE_ROLE},
    bolt_v3_decision_evidence::{
        BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_CURRENT,
        BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_NEXT, BoltV3OrderIntentEvidence,
        BoltV3OrderIntentKind, BoltV3ReadinessGateEvidenceSnapshot, BoltV3RuntimeReadinessSeed,
        BoltV3StrategyInputEvidenceSnapshot, compiled_order_price_source,
        validate_readiness_gate_evidence_snapshot,
    },
    bolt_v3_market_families::{
        self, FairProbabilityInputs, MarketSelectionOutcome, MarketSelectionTarget, OutcomeSide,
        SelectedMarketSourceIdentity,
    },
    bolt_v3_numeric::{
        BPS_DENOMINATOR, MIDPOINT_DIVISOR_F64, MILLIS_PER_SECOND_U64, UNIT_F64, ZERO_F64,
        clamp_probability, is_non_negative_finite, is_positive_finite, sanitize_probability,
    },
    bolt_v3_operator_artifacts::{EntryReadinessGateSession, GateSatisfaction},
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate, build_nt_order},
    bolt_v3_position_contract::{
        expected_exit_order_side_for_position, expected_position_side_for_entry_order,
        is_observed_open_side,
    },
    bolt_v3_providers::normalize_base_order_quantity_for_execution_venue as provider_normalize_base_order_quantity,
    bolt_v3_submit_admission::{
        BoltV3CompiledOrderKind, BoltV3CompiledOrderLiquidity, BoltV3CompiledOrderSide,
        BoltV3CompiledOrderSizingEvidence, BoltV3CompiledProductKind, BoltV3RiskReducingExitProof,
        BoltV3SubmitAdmissionRequest, BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy,
        PredictionMarketOutcomeSide, admission_base_notional_from_order,
        base_quantity_admission_notional, fee_inclusive_admission_notional,
        market_style_admission_ceiling_notional,
    },
    bolt_v3_taker_signal::{
        RobustSizingInputs, SideSelectionInputs, ThetaScalerInputs, UncertaintyBandInputs,
        WorstCaseEvInputs, choose_entry_side, choose_robust_size, compute_theta_scaler,
        compute_worst_case_ev_bps, outcome_side_evidence_label, price_agreement_corr,
        price_gap_probability, uncertainty_band_probability,
    },
    bolt_v3_volatility::{RealizedVolConfig, RealizedVolEstimator},
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
            use_uuid_client_order_ids: bool => Boolean;
            use_hyphens_in_client_order_ids: bool => Boolean;
            external_order_claims: Vec<String> => Array;
            manage_contingent_orders: bool => Boolean;
            manage_gtd_expiry: bool => Boolean;
            manage_stop: bool => Boolean;
            market_exit_interval_ms: u64 => Integer;
            market_exit_max_attempts: u64 => Integer;
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
            trade_flow_window_secs: u64 => Integer;
            trade_flow_max_samples: u64 => Integer;
            spike_guard_return_threshold: f64 => Float;
            spike_guard_cooldown_secs: u64 => Integer;
            price_to_beat_source: String => String;
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
    order_type: OrderType,
    time_in_force: TimeInForce,
    expire_time_unix_nanos: Option<u64>,
    trigger_price: Option<f64>,
    activation_price: Option<f64>,
    trigger_type: Option<TriggerType>,
    trigger_instrument_id: Option<InstrumentId>,
    trailing_offset: Option<f64>,
    trailing_offset_type: Option<TrailingOffsetType>,
    is_post_only: bool,
    is_reduce_only: bool,
    is_quote_quantity: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ConfiguredNtOrderTemplate {
    order_type: OrderType,
    time_in_force: TimeInForce,
    expire_time_unix_nanos: Option<u64>,
    trigger_price: Option<f64>,
    activation_price: Option<f64>,
    trigger_type: Option<TriggerType>,
    trigger_instrument_id: Option<InstrumentId>,
    trailing_offset: Option<f64>,
    trailing_offset_type: Option<TrailingOffsetType>,
    is_post_only: bool,
    is_reduce_only: bool,
    is_quote_quantity: bool,
}

impl ConfiguredNtOrderTemplate {
    fn nt_order_template(
        &self,
        prefix: &'static str,
        price_precision: u8,
    ) -> Result<NtOrderTemplate> {
        Ok(NtOrderTemplate {
            order_type: self.order_type,
            time_in_force: self.time_in_force,
            expire_time: expire_time_from_config(self.expire_time_unix_nanos),
            trigger_price: trigger_price_from_config(prefix, self.trigger_price, price_precision)?,
            activation_price: activation_price_from_config(
                prefix,
                self.activation_price,
                price_precision,
            )?,
            trigger_type: self.trigger_type,
            trigger_instrument_id: self.trigger_instrument_id,
            trailing_offset: trailing_offset_from_config(prefix, self.trailing_offset)?,
            trailing_offset_type: self.trailing_offset_type,
            is_post_only: self.is_post_only,
            is_reduce_only: self.is_reduce_only,
            is_quote_quantity: self.is_quote_quantity,
        })
    }
}

impl From<&BinaryOracleEdgeTakerOrderConfig> for ConfiguredNtOrderTemplate {
    fn from(order: &BinaryOracleEdgeTakerOrderConfig) -> Self {
        Self {
            order_type: order.order_type,
            time_in_force: order.time_in_force,
            expire_time_unix_nanos: order.expire_time_unix_nanos,
            trigger_price: order.trigger_price,
            activation_price: order.activation_price,
            trigger_type: order.trigger_type,
            trigger_instrument_id: order.trigger_instrument_id,
            trailing_offset: order.trailing_offset,
            trailing_offset_type: order.trailing_offset_type,
            is_post_only: order.is_post_only,
            is_reduce_only: order.is_reduce_only,
            is_quote_quantity: order.is_quote_quantity,
        }
    }
}

#[derive(Debug, Clone)]
struct SubmitContext {
    client_id: Option<ClientId>,
    position_id: Option<PositionId>,
    params: Option<Params>,
}

impl SubmitContext {
    fn from_parts(
        client_id: Option<ClientId>,
        position_id: Option<PositionId>,
        params: Option<Params>,
    ) -> Self {
        Self {
            client_id,
            position_id,
            params,
        }
    }

    fn with_client_id(client_id: ClientId) -> Self {
        Self::from_parts(Some(client_id), None, None)
    }

    fn with_client_id_and_position_id(client_id: ClientId, position_id: PositionId) -> Self {
        Self::from_parts(Some(client_id), Some(position_id), None)
    }
}

impl BinaryOracleEdgeTakerOrderConfig {
    fn nt_order_template(
        &self,
        prefix: &'static str,
        price_precision: u8,
    ) -> Result<NtOrderTemplate> {
        ConfiguredNtOrderTemplate::from(self).nt_order_template(prefix, price_precision)
    }
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
            reference_venue: Option<String>,
            reference_instrument_id: Option<String>,
            entry_order: BinaryOracleEdgeTakerOrderConfig,
            exit_order: BinaryOracleEdgeTakerOrderConfig,
            forced_exit_order: BinaryOracleEdgeTakerOrderConfig,
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

const ORDER_EXPIRE_TIME_UNIX_NANOS_FIELD: &str = "expire_time_unix_nanos";
const ORDER_TRIGGER_PRICE_FIELD: &str = "trigger_price";
const ORDER_ACTIVATION_PRICE_FIELD: &str = "activation_price";
const ORDER_TRIGGER_TYPE_FIELD: &str = "trigger_type";
const ORDER_TRIGGER_INSTRUMENT_ID_FIELD: &str = "trigger_instrument_id";
const ORDER_TRAILING_OFFSET_FIELD: &str = "trailing_offset";
const ORDER_TRAILING_OFFSET_TYPE_FIELD: &str = "trailing_offset_type";

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
    source_identity: SelectedMarketSourceIdentity,
    selection_outcome: MarketSelectionOutcome,
    price_to_beat: Option<f64>,
    start_ts_ms: u64,
    expiration_ts_ms: u64,
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

    /// Whether this book is priced and strictly crossed (`best_bid > best_ask`).
    ///
    /// A locked book (`best_bid == best_ask`) is not crossed and is intentionally
    /// not flagged. An unpriced book (either side missing) is not crossed either;
    /// the [`EntryBlockReason::ActiveBookNotPriced`] gate already covers that case.
    fn is_crossed(&self) -> bool {
        matches!(
            (self.best_bid, self.best_ask),
            (Some(best_bid), Some(best_ask)) if best_bid > best_ask
        )
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

    fn passive_price_for_order_side(&self, order_side: OrderSide) -> Option<f64> {
        match order_side {
            OrderSide::Buy => self.best_bid,
            OrderSide::Sell => self.best_ask,
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

    /// Whether either active outcome book is priced and strictly crossed.
    ///
    /// Mirrors how [`OutcomePreparedBooks::is_priced`] treats both outcome books
    /// as the active book for the entry gate: a cross on either side is an invalid
    /// market state that must block entry.
    fn any_crossed(&self) -> bool {
        self.up.is_crossed() || self.down.is_crossed()
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
    source_identity: Option<SelectedMarketSourceIdentity>,
    instrument_id: Option<InstrumentId>,
    outcome_fees: OutcomeFeeState,
    price_to_beat: Option<f64>,
    market_selection_outcome: MarketSelectionOutcome,
    interval_start_ms: Option<u64>,
    interval_end_ms: Option<u64>,
    selection_published_at_ms: Option<u64>,
    seconds_to_expiry_at_selection: Option<u64>,
    interval_open: Option<f64>,
    last_reference_ts_ms: Option<u64>,
    warmup_count: u64,
    warmup_target: u64,
    books: OutcomePreparedBooks,
    trade_flow: BTreeMap<InstrumentId, SignedTradeFlow>,
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

#[derive(Debug, Clone, Copy, PartialEq)]
struct ImpactCappedExecution {
    quantity: f64,
    vwap_price: f64,
}

/// A single signed trade retained for downstream adverse-selection / VPIN analysis.
///
/// This is the per-trade element stored by [`SignedTradeFlow`]; the signed
/// aggressor side, price, and size are the inputs the W3 Glosten-Milgrom / VPIN
/// stage will read. Fields are public because this struct is the read seam for
/// that later stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SignedTrade {
    pub ts_ms: u64,
    pub aggressor: AggressorSide,
    pub price: f64,
    pub size: f64,
}

/// Runtime view of the trade-flow buffer's window/cap knobs.
///
/// Plain runtime config view without a serde derive: the strategy owns TOML
/// deserialization and projects the relevant fields here at the call site
/// (mirroring [`RealizedVolConfig`] for the volatility estimator), so the buffer
/// never depends on the strategy's config type and moves cleanly into a shared
/// module when the W3 consumer is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignedTradeFlowConfig {
    pub window_secs: u64,
    pub max_samples: u64,
}

/// Project the strategy's trade-flow TOML knobs into the buffer's runtime config
/// view. Single place that maps those fields onto [`SignedTradeFlowConfig`]
/// (mirrors [`realized_vol_config`]).
fn signed_trade_flow_config(config: &BinaryOracleEdgeTakerConfig) -> SignedTradeFlowConfig {
    SignedTradeFlowConfig {
        window_secs: config.trade_flow_window_secs,
        max_samples: config.trade_flow_max_samples,
    }
}

/// Bounded rolling buffer of signed trades for a single quoted instrument.
///
/// Mirrors [`RealizedVolEstimator`]'s config-driven rolling-window shape: the
/// retention window and hard sample cap both come from a projected
/// [`SignedTradeFlowConfig`], and the buffer is bounded by time (`window_ms`) and
/// count (`max_samples`). It only retains signed trade flow; the W3 stage reads
/// it to compute adverse-selection signals.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedTradeFlow {
    window_ms: u64,
    max_samples: usize,
    samples: VecDeque<SignedTrade>,
}

impl SignedTradeFlow {
    fn from_config(config: &SignedTradeFlowConfig) -> Self {
        Self {
            window_ms: config.window_secs.saturating_mul(MILLIS_PER_SECOND_U64),
            max_samples: config.max_samples as usize,
            samples: VecDeque::new(),
        }
    }

    fn observe(&mut self, trade: &TradeTick) {
        let ts_ms = trade.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        // Reject non-monotonic observations, mirroring `RealizedVolEstimator`: a
        // timestamp at or before the latest retained sample would break the
        // oldest-first ordering and the time-window eviction cutoff.
        if self
            .samples
            .back()
            .is_some_and(|previous| ts_ms <= previous.ts_ms)
        {
            return;
        }
        self.samples.push_back(SignedTrade {
            ts_ms,
            aggressor: trade.aggressor_side,
            price: trade.price.as_f64(),
            size: trade.size.as_f64(),
        });
        self.evict(ts_ms);
    }

    fn evict(&mut self, now_ms: u64) {
        let cutoff_ms = now_ms.saturating_sub(self.window_ms);
        while self
            .samples
            .front()
            .is_some_and(|trade| trade.ts_ms < cutoff_ms)
        {
            let _ = self.samples.pop_front();
        }
        while self.samples.len() > self.max_samples {
            let _ = self.samples.pop_front();
        }
    }

    /// Number of signed trades currently retained. Read seam for the W3 stage.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether the buffer currently holds no retained trades.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Retained signed trades, oldest first. Read seam for the W3 stage.
    ///
    /// These are evicted only as of the last [`observe`](Self::observe); in a
    /// quiet market some may have aged out of the window. A point-in-time
    /// consumer should read through [`samples_within`](Self::samples_within).
    pub fn samples(&self) -> &VecDeque<SignedTrade> {
        &self.samples
    }

    /// Signed trades still inside the retention window as of `now_ms`, oldest
    /// first. Unlike [`samples`](Self::samples) this filters against the caller's
    /// clock rather than the last `observe` timestamp, so a quiet market does not
    /// surface trades that have aged out of the window. Read seam for the W3
    /// stage's point-in-time adverse-selection reads.
    pub fn samples_within(&self, now_ms: u64) -> impl Iterator<Item = &SignedTrade> {
        let cutoff_ms = now_ms.saturating_sub(self.window_ms);
        self.samples
            .iter()
            .filter(move |trade| trade.ts_ms >= cutoff_ms)
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
    /// Reference-spot spike cooldown deadline (ms). When set, entry is blocked
    /// until `now_ms >= spike_until_ms`. Set when a single-step reference-spot
    /// move clears the configured spike threshold; `None` outside any cooldown.
    spike_until_ms: Option<u64>,
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
    terminal_received: bool,
    residual_position_observed_after_fill: bool,
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
    pending_entry: Option<PendingEntryState>,
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

    fn into_state_after_exit_update(self) -> ExposureState {
        if self.is_terminal() {
            return ExposureState::Flat;
        }
        if self.pending_exit.terminal_received
            && (!self.pending_exit.fill_received
                || self.pending_exit.residual_position_observed_after_fill)
        {
            return match self.position {
                Some(position) => ExposureState::Managed(position),
                None => ExposureState::Flat,
            };
        }
        ExposureState::ExitPending(self)
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
    ForeignVenuePosition {
        instrument_venue: Venue,
        execution_venue: Venue,
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
            Self::Managed(position) => position.pending_entry.as_ref(),
            Self::ExitPending(exit) => exit
                .position
                .as_ref()
                .and_then(|position| position.pending_entry.as_ref()),
            _ => None,
        }
    }

    fn pending_entry_mut(&mut self) -> Option<&mut PendingEntryState> {
        match self {
            Self::PendingEntry(pending) | Self::EntryReconcilePending { pending, .. } => {
                Some(pending)
            }
            Self::Managed(position) => position.pending_entry.as_mut(),
            Self::ExitPending(exit) => exit
                .position
                .as_mut()
                .and_then(|position| position.pending_entry.as_mut()),
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

    fn managed_position_mut(&mut self) -> Option<&mut ManagedPositionState> {
        match self {
            Self::Managed(position) => Some(position),
            Self::ExitPending(exit) => exit.position.as_mut(),
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

    fn held_instrument_id(&self) -> Option<InstrumentId> {
        self.observed_position()
            .map(|position| position.instrument_id)
            .or_else(|| self.pending_entry().map(|pending| pending.instrument_id))
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
    if contract.entry_position_side == PositionSide::Short
        || contract.exit_position_side == PositionSide::Short
    {
        return false;
    }
    expected_position_side_for_entry_order(contract.entry_order_side)
        .is_some_and(|side| side == contract.entry_position_side)
        && expected_exit_order_side_for_position(contract.exit_position_side)
            .is_some_and(|side| side == contract.exit_order_side)
        && contract.entry_position_side == contract.exit_position_side
        && is_observed_open_side(contract.entry_position_side)
}

fn order_price_for_side(
    book: &OutcomeBookState,
    order_side: OrderSide,
    is_post_only: bool,
) -> Option<f64> {
    if is_post_only {
        book.passive_price_for_order_side(order_side)
    } else {
        book.executable_price_for_order_side(order_side)
    }
}

fn visible_book_depth_side_for_order(
    order_side: OrderSide,
    is_post_only: bool,
) -> Option<OrderSide> {
    match (order_side, is_post_only) {
        (OrderSide::Buy, false) | (OrderSide::Sell, true) => Some(OrderSide::Buy),
        (OrderSide::Sell, false) | (OrderSide::Buy, true) => Some(OrderSide::Sell),
        _ => None,
    }
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

/// Project the strategy's volatility-window TOML knobs into the foundational
/// estimator's runtime config view. The strategy owns deserialization; this is
/// the single place that maps those fields onto [`RealizedVolConfig`].
fn realized_vol_config(config: &BinaryOracleEdgeTakerConfig) -> RealizedVolConfig {
    RealizedVolConfig {
        window_secs: config.vol_window_secs,
        gap_reset_secs: config.vol_gap_reset_secs,
        min_observations: config.vol_min_observations,
        bridge_valid_secs: config.vol_bridge_valid_secs,
    }
}

impl PricingState {
    fn from_config(config: &BinaryOracleEdgeTakerConfig) -> Self {
        Self {
            last_reference_fair_value: None,
            fast_spot: None,
            realized_vol: RealizedVolEstimator::from_config(&realized_vol_config(config)),
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
            spike_until_ms: None,
        }
    }

    fn observe_reference_quote(
        &mut self,
        quote: &FastSpotObservation,
        min_agreement_corr: f64,
        max_jitter_ms: u64,
        spike_return_threshold: f64,
        spike_cooldown_secs: u64,
    ) {
        if !is_positive_finite(quote.price) {
            return;
        }

        self.detect_reference_spike(quote, spike_return_threshold, spike_cooldown_secs);

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
                let _ = estimator.observe(&quote.venue_name, quote.price, quote.observed_ts_ms);
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

    /// Arm the spike cooldown when a new reference-spot observation jumps past
    /// the configured single-step return threshold.
    ///
    /// Reads the still-current `fast_spot` as the previous observation, before
    /// `observe_reference_quote` overwrites it. The single-step relative move is
    /// `m = (new_price / prev_price - 1).abs()`; when `m >=
    /// spike_return_threshold` the cooldown deadline is extended to the later of
    /// its current value and `new_observed_ts_ms + spike_cooldown_secs *
    /// MILLIS_PER_SECOND_U64`, so an out-of-order quote can never retract an
    /// active cooldown. With no valid previous observation there is no spike.
    /// This is an additive read of
    /// previous vs new and does not alter `fast_spot` or realized-vol behavior.
    fn detect_reference_spike(
        &mut self,
        quote: &FastSpotObservation,
        spike_return_threshold: f64,
        spike_cooldown_secs: u64,
    ) {
        let Some(previous) = self.fast_spot.as_ref() else {
            return;
        };
        if !is_positive_finite(previous.price) || !is_positive_finite(quote.price) {
            return;
        }
        let relative_move = (quote.price / previous.price - UNIT_F64).abs();
        if relative_move >= spike_return_threshold {
            let new_deadline = quote
                .observed_ts_ms
                .saturating_add(spike_cooldown_secs.saturating_mul(MILLIS_PER_SECOND_U64));
            // Fail closed: the cooldown deadline only ever extends. An
            // out-of-order spike carrying an earlier timestamp must never shorten
            // an active cooldown and re-enable entry during volatility.
            self.spike_until_ms = Some(match self.spike_until_ms {
                Some(existing) => existing.max(new_deadline),
                None => new_deadline,
            });
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
            let _ = estimator.observe(&candidate.venue_name, price, observed_ts_ms);
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
                .or_else(|| self.realized_vol.active_venue.clone()),
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
            source_identity: None,
            instrument_id: None,
            outcome_fees: OutcomeFeeState::empty(),
            price_to_beat: None,
            market_selection_outcome: MarketSelectionOutcome::Current,
            interval_start_ms: None,
            interval_end_ms: None,
            selection_published_at_ms: None,
            seconds_to_expiry_at_selection: None,
            interval_open: None,
            last_reference_ts_ms: None,
            warmup_count: INITIAL_COUNTER_U64,
            warmup_target: INITIAL_COUNTER_U64,
            books: OutcomePreparedBooks::empty(),
            trade_flow: BTreeMap::new(),
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
            source_identity: Some(market.source_identity.clone()),
            instrument_id: Some(InstrumentId::from(market.instrument_id.as_str())),
            outcome_fees: OutcomeFeeState::from_market(market),
            price_to_beat: market.price_to_beat,
            market_selection_outcome: market.selection_outcome,
            interval_start_ms: Some(market.start_ts_ms),
            interval_end_ms: Some(market.expiration_ts_ms),
            selection_published_at_ms: None,
            seconds_to_expiry_at_selection: Some(market.seconds_to_end),
            interval_open: None,
            last_reference_ts_ms: None,
            warmup_count: INITIAL_COUNTER_U64,
            warmup_target,
            books: OutcomePreparedBooks::from_market(market),
            trade_flow: BTreeMap::new(),
            fast_venue_incoherent: false,
            forced_flat,
        }
    }

    fn same_boundary(&self, other: &Self) -> bool {
        self.phase == other.phase
            && self.market_id == other.market_id
            && self.instrument_id == other.instrument_id
            && self.market_selection_outcome == other.market_selection_outcome
            && self.interval_start_ms == other.interval_start_ms
            && self.interval_end_ms == other.interval_end_ms
    }

    fn warmup_complete(&self) -> bool {
        self.warmup_target > INITIAL_COUNTER_U64 && self.warmup_count >= self.warmup_target
    }

    fn apply_selection_timing(&mut self, snapshot: &RuntimeSelectionSnapshot) {
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
        let Some(anchor_price) = self
            .price_to_beat
            .filter(|value| is_positive_finite(*value))
        else {
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
        let market_exit_time_in_force = config.forced_exit_order.time_in_force;
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
                market_exit_reduce_only: config.forced_exit_order.is_reduce_only,
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
        self.apply_source_owned_readiness_seed();
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

    fn apply_source_owned_readiness_seed(&mut self) {
        let Some(seed) = self.context.runtime_readiness_seed().cloned() else {
            return;
        };
        if !self.source_owned_readiness_seed_matches_active(&seed) {
            return;
        }
        if !is_positive_finite(seed.price_to_beat_value)
            || !is_positive_finite(seed.reference_price)
            || !is_positive_finite(seed.realized_volatility)
        {
            return;
        }

        self.active.price_to_beat = Some(seed.price_to_beat_value);
        self.observe_reference_quote(&FastSpotObservation {
            venue_name: seed.reference_venue.clone(),
            price: seed.reference_price,
            observed_ts_ms: seed.reference_quote_ts_event,
        });
        self.active.warmup_count = self.active.warmup_count.max(self.active.warmup_target);
        if self
            .pricing
            .realized_vol
            .last_ready_ts_ms
            .is_none_or(|ready_ts_ms| ready_ts_ms <= seed.reference_quote_ts_event)
        {
            self.pricing.realized_vol.last_ready_vol = Some(seed.realized_volatility);
            self.pricing.realized_vol.last_ready_ts_ms = Some(seed.reference_quote_ts_event);
            self.pricing.realized_vol_source_venue = Some(seed.reference_venue);
        }
    }

    fn source_owned_readiness_seed_matches_active(
        &self,
        seed: &BoltV3RuntimeReadinessSeed,
    ) -> bool {
        if self.active.phase != SelectionPhase::Active {
            return false;
        }
        let Some(identity) = self.active.source_identity.as_ref() else {
            return false;
        };
        if identity.condition_id != seed.polymarket_condition_id
            || identity.market_slug != seed.polymarket_market_slug
            || identity.question_id != seed.polymarket_question_id
        {
            return false;
        }
        if self.active.interval_start_ms != Some(seed.market_start_timestamp_ms)
            || self.active.interval_end_ms != Some(seed.market_end_timestamp_ms)
        {
            return false;
        }
        let Some(up_instrument_id) = self.active.books.up.instrument_id else {
            return false;
        };
        let Some(down_instrument_id) = self.active.books.down.instrument_id else {
            return false;
        };
        up_instrument_id.to_string() == seed.up_instrument_id
            && down_instrument_id.to_string() == seed.down_instrument_id
    }

    fn observe_reference_quote(&mut self, quote: &FastSpotObservation) {
        self.active.observe_reference_quote(quote);
        self.pricing.observe_reference_quote(
            quote,
            self.config.lead_agreement_min_corr,
            self.config.lead_jitter_max_ms,
            self.config.spike_guard_return_threshold,
            self.config.spike_guard_cooldown_secs,
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
        let venue_name = self.config.reference_venue.as_ref()?;
        Some(FastSpotObservation {
            venue_name: venue_name.clone(),
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
        // Scope market selection to the configured execution venue. The shared NT cache can hold
        // instruments from EVERY registered data client (the shipped config registers several
        // non-Polymarket reference venues, and `load_state` can repopulate the cache across runs),
        // so an unscoped read could feed a foreign-venue instrument into selection. A real order can
        // only ever route to the execution client's venue, so any market on another venue must be
        // unselectable here. Filtering the cache read by the execution venue makes a wrong-venue
        // selection structurally impossible and fails closed (P5-5 / Codex P5).
        let execution_venue = self.context.execution_venue();
        let instruments = {
            let cache = self.cache();
            cache
                .instrument_ids(None)
                .into_iter()
                .filter_map(|instrument_id| cache.instrument(instrument_id).cloned())
                .filter(|instrument| instrument.id().venue == execution_venue)
                .collect::<Vec<_>>()
        };
        let snapshot = selection_snapshot_from_instruments(&self.config, &instruments, now_ms);
        // Defense in depth: the venue-scoped read above cannot yield a foreign-venue market, but
        // assert it explicitly so any future widening of the read still fails closed here — a real
        // order must never fire against a market whose outcome instruments are on a venue other than
        // the execution venue.
        let snapshot = if selected_market_on_execution_venue(&snapshot, execution_venue) {
            snapshot
        } else {
            log::error!(
                "binary_oracle_edge_taker refusing a selected market whose outcome venue is not the execution venue: strategy_id={} execution_venue={}",
                self.config.strategy_id,
                execution_venue,
            );
            idle_selection_snapshot(
                &self.config,
                now_ms,
                SELECTION_BLOCK_REASON_TARGET_SELECTION_BLOCKED,
            )
        };
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

    fn reference_instrument_id(&self) -> Option<InstrumentId> {
        self.config
            .reference_instrument_id
            .as_deref()
            .and_then(|instrument_id| InstrumentId::from_str(instrument_id).ok())
    }

    fn subscribe_reference_quotes(&mut self) {
        if let Some(instrument_id) = self.reference_instrument_id() {
            #[cfg(not(test))]
            self.subscribe_quotes(instrument_id, None, None);
            #[cfg(test)]
            let _ = instrument_id;
        }
    }

    fn unsubscribe_reference_quotes(&mut self) {
        if let Some(instrument_id) = self.reference_instrument_id() {
            #[cfg(not(test))]
            self.unsubscribe_quotes(instrument_id, None, None);
            #[cfg(test)]
            let _ = instrument_id;
        }
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

    fn entry_order_may_remain_working(&self, client_order_id: &ClientOrderId) -> bool {
        if !matches!(
            self.config.entry_order.time_in_force,
            TimeInForce::Gtc | TimeInForce::Gtd
        ) {
            return false;
        }

        let cached_closed = if self.is_registered() {
            self.cache()
                .order(client_order_id)
                .map(|order| order.is_closed())
        } else {
            None
        };

        match cached_closed {
            Some(closed) => !closed,
            None => true,
        }
    }

    fn clear_managed_pending_entry_for_client_order(
        &mut self,
        client_order_id: ClientOrderId,
        event_instrument_id: InstrumentId,
    ) {
        let matches_pending_entry = self
            .exposure
            .managed_position()
            .and_then(|managed| managed.pending_entry.as_ref())
            .is_some_and(|pending| pending.client_order_id == client_order_id);
        if !matches_pending_entry {
            return;
        }
        if !self.event_instrument_matches_held_exposure(event_instrument_id) {
            return;
        }
        if let Some(managed) = self.exposure.managed_position_mut() {
            managed.pending_entry = None;
            self.prune_market_lifecycle_at_current_time();
        }
    }

    fn clear_pending_entry_for_client_order(
        &mut self,
        client_order_id: ClientOrderId,
        event_instrument_id: InstrumentId,
    ) {
        let matches_pending_entry = matches!(
            &self.exposure,
            ExposureState::PendingEntry(pending) if pending.client_order_id == client_order_id
        );
        if matches_pending_entry {
            if !self.event_instrument_matches_held_exposure(event_instrument_id) {
                return;
            }
            self.exposure = ExposureState::Flat;
            self.prune_market_lifecycle_at_current_time();
            return;
        }

        self.clear_managed_pending_entry_for_client_order(client_order_id, event_instrument_id);
    }

    fn prune_market_lifecycle_at_current_time(&mut self) {
        if self.is_registered() {
            let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
            self.prune_market_lifecycle(now_ms);
        }
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
        // Scope recovery to the configured execution venue. The shared NT cache can hold positions
        // from every registered execution client; a foreign-venue position must never be accepted
        // into Managed state because the exit path would build/submit an order for it with no
        // additional venue gate. Filtering the cache read by execution venue makes a wrong-venue
        // recovery structurally impossible and fails closed (P5-5 / Codex P5).
        let strategy_id = StrategyId::from(self.config.strategy_id.as_str());
        let execution_venue = self.context.execution_venue();
        let cached_positions = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let cache = self.cache();
            cache
                .positions_open(Some(&execution_venue), None, Some(&strategy_id), None, None)
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
        let exposure = self.bootstrapped_exposure_for(open_position, execution_venue);
        self.exposure = exposure;
    }

    /// Classify a single recovered open position into the exposure state to adopt.
    ///
    /// The recovery probe already scopes the cache read to the execution venue, so a
    /// foreign-venue position should never reach here in production. This is the single
    /// fail-closed adoption decision and re-asserts the venue invariant structurally
    /// (defense in depth) BEFORE any contract check: the exit path would otherwise
    /// build/submit an order for a wrong-venue position with no further venue gate
    /// (P5-5 / Codex P5). A foreign-venue position is quarantined to blind recovery and
    /// is never managed.
    fn bootstrapped_exposure_for(
        &self,
        open_position: OpenPositionState,
        execution_venue: Venue,
    ) -> ExposureState {
        if open_position.instrument_id.venue != execution_venue {
            log::error!(
                "binary_oracle_edge_taker recovery bootstrap quarantined foreign-venue cached position: strategy_id={} position_id={} instrument_id={} instrument_venue={} execution_venue={}",
                self.config.strategy_id,
                open_position.position_id,
                open_position.instrument_id,
                open_position.instrument_id.venue,
                execution_venue,
            );
            return ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::ForeignVenuePosition {
                    instrument_venue: open_position.instrument_id.venue,
                    execution_venue,
                },
            });
        }
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
            ExposureState::Managed(ManagedPositionState {
                position: open_position,
                origin: ManagedPositionOrigin::RecoveryBootstrap,
                pending_entry: None,
            })
        } else if is_observed_open_side(open_position.side) {
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
            ExposureState::UnsupportedObserved(UnsupportedObservedState {
                observed: open_position,
                reason: UnsupportedObservedReason::BootstrappedUnsupportedContract,
            })
        } else {
            log::error!(
                "binary_oracle_edge_taker recovery bootstrap received invalid cached position side: strategy_id={} position_id={} instrument_id={} entry_order_side={:?} side={:?}",
                self.config.strategy_id,
                open_position.position_id,
                open_position.instrument_id,
                open_position.entry_order_side,
                open_position.side,
            );
            ExposureState::BlindRecovery(BlindRecoveryState {
                reason: BlindRecoveryReason::InvalidBootstrappedPosition {
                    entry_order_side: open_position.entry_order_side,
                    side: open_position.side,
                },
            })
        }
    }

    fn exposure_occupancy(&self) -> Option<ExposureOccupancy> {
        self.exposure.occupancy()
    }

    fn clear_pending_entry_state(&mut self) {
        if matches!(self.exposure, ExposureState::PendingEntry(_)) {
            self.exposure = ExposureState::Flat;
            self.prune_market_lifecycle_at_current_time();
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
        if self.active.books.any_crossed() {
            blocked_by.push(EntryBlockReason::BookCrossed);
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
        if self
            .pricing
            .spike_until_ms
            .is_some_and(|spike_until_ms| now_ms < spike_until_ms)
        {
            blocked_by.push(EntryBlockReason::SpotSpikeCooldown);
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

    /// Single source of truth for the reference-quote freshness bound enforced
    /// by the forced-flat stale check (A5). When the live canary gate is armed,
    /// the gate-approved `[live_canary].reference_quote_max_age_seconds` is the
    /// authoritative freshness policy for the live path, so it is plumbed in here
    /// rather than letting the submit/stale path run an independent
    /// strategy-config policy. The effective bound is the STRICTER (smaller) of
    /// the gate-approved bound and the strategy's `forced_flat_stale_reference_ms`
    /// so plumbing the gate value can only ever TIGHTEN the existing guard, never
    /// loosen it (a larger gate bound never relaxes the strategy bound, and the
    /// strategy bound never relaxes the gate's). Pre-arm there is no gate bound
    /// and no order can be submitted anyway, so the strategy bound alone applies.
    fn effective_stale_reference_after_ms(&self) -> u64 {
        let config_bound_ms = self.config.forced_flat_stale_reference_ms;
        match self
            .context
            .submit_admission()
            .reference_quote_max_age_seconds()
        {
            Some(gate_seconds) => {
                config_bound_ms.min(gate_seconds.saturating_mul(MILLIS_PER_SECOND_U64))
            }
            None => config_bound_ms,
        }
    }

    fn active_forced_flat_reasons_at(&self, now_ms: u64) -> Vec<ForcedFlatReason> {
        evaluate_forced_flat_predicates(&ForcedFlatInputs {
            phase: self.active.phase,
            metadata_matches_selection: self.active.books.metadata_matches_selection(),
            last_reference_ts_ms: self.active.last_reference_ts_ms,
            now_ms,
            stale_reference_after_ms: self.effective_stale_reference_after_ms(),
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
            stale_reference_after_ms: self.effective_stale_reference_after_ms(),
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
                seconds_to_market_end: seconds_to_expiry,
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
        bolt_v3_market_families::fair_probability_up_for_family(
            &self.config.rotating_market_family,
            &FairProbabilityInputs {
                spot_price: inputs.spot_price,
                strike_price: inputs.strike_price,
                seconds_to_market_end: inputs.seconds_to_expiry,
                realized_vol: inputs.realized_vol,
                pricing_kurtosis: self.config.pricing_kurtosis,
            },
        )
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
            seconds_to_market_end: self.current_seconds_to_expiry_at(now_ms)?,
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
            up_fee_bps: self.entry_fee_bps(OutcomeSide::Up),
            down_fee_bps: self.entry_fee_bps(OutcomeSide::Down),
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

    fn entry_fee_bps(&self, side: OutcomeSide) -> Option<f64> {
        let entry_price = self.executable_entry_cost(side)?;
        self.entry_fee_bps_at_price(side, entry_price)
    }

    fn entry_fee_bps_at_price(&self, side: OutcomeSide, entry_price: f64) -> Option<f64> {
        let instrument_id = self.instrument_id_for_side(side)?;
        let Some(instrument) = self.current_instrument(instrument_id) else {
            #[cfg(test)]
            {
                return self.context.fee_provider().fee_bps(instrument_id)?.to_f64();
            }
            #[cfg(not(test))]
            {
                return None;
            }
        };
        let entry_price = Decimal::from_f64(entry_price)?;
        self.context
            .fee_provider()
            .entry_fee_bps(&instrument, entry_price)?
            .to_f64()
    }

    /// Resolve the max entry fee bound (in bps) used to compute the
    /// fee-inclusive admission notional, mirroring the proof executor's
    /// `fee_provider.max_entry_fee_bps(&instrument, price)` lookup
    /// (`bolt_v3_canary_proof_executor` line 108-116). Fail-closed: a missing
    /// instrument context or absent fee bound is a hard error so the
    /// downstream cap check never silently passes a raw notional.
    ///
    /// In production the order is built from a cached instrument, so
    /// `current_instrument` resolves. Unit tests that exercise this path
    /// without populating the NT cache supply the bound through the
    /// `FeeProvider::fee_bps` map, matching the established `#[cfg(test)]`
    /// fallback in `entry_fee_bps_at_price`.
    ///
    /// SYMMETRIC-FEE ASSUMPTION (A12): both entry AND risk-reducing-exit
    /// admission scale their notional by THIS entry-fee bound. Polymarket
    /// charges the same fee on either leg, so the entry bound is the exact
    /// exit bound today. Should a venue ever charge a strictly larger exit fee,
    /// using the (smaller) entry bound here would UNDERSTATE an exit's
    /// fee-inclusive notional. That direction fails OPEN for the cap, so a
    /// venue with asymmetric (higher exit) fees MUST add an exit-fee bound and
    /// route exits through it before being admitted — do not silently reuse the
    /// entry bound for an asymmetric-fee venue.
    fn max_entry_fee_bps_for_admission(
        &self,
        instrument_id: InstrumentId,
        price: Decimal,
    ) -> Result<Decimal> {
        let max_fee_bps = match self.current_instrument(instrument_id) {
            Some(instrument) => self.context.fee_provider().max_entry_fee_bps(&instrument, price),
            None => {
                #[cfg(test)]
                {
                    self.context.fee_provider().fee_bps(instrument_id)
                }
                #[cfg(not(test))]
                {
                    None
                }
            }
        }
        .with_context(|| {
            format!(
                "bolt-v3 submit admission requires a max entry fee bound for instrument_id={instrument_id}"
            )
        })?;
        anyhow::ensure!(
            max_fee_bps >= Decimal::ZERO,
            "bolt-v3 submit admission max entry fee bound must be non-negative for instrument_id={instrument_id}, got {max_fee_bps}"
        );
        Ok(max_fee_bps)
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
        if matches!(
            self.config.entry_order.order_type,
            OrderType::StopMarket | OrderType::MarketIfTouched | OrderType::TrailingStopMarket
        ) {
            return self
                .config
                .entry_order
                .trigger_price
                .or(self.config.entry_order.activation_price)
                .filter(|value| is_positive_finite(*value));
        }
        let order_side = self.configured_entry_order_side().ok()?;
        let book = self.active_book_for_outcome(side);
        order_price_for_side(book, order_side, self.config.entry_order.is_post_only)
    }

    fn submission_entry_price(&self, side: OutcomeSide) -> Option<f64> {
        self.executable_entry_cost(side)
    }

    fn visible_book_notional_cap(&self, side: OutcomeSide) -> Option<f64> {
        let order_side = self.configured_entry_order_side().ok()?;
        let book_depth_side =
            visible_book_depth_side_for_order(order_side, self.config.entry_order.is_post_only)?;
        let capped_execution = self
            .active_book_for_outcome(side)
            .max_execution_within_vwap_slippage_bps(
                book_depth_side,
                self.config.book_impact_cap_bps,
            )
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

    fn normalize_base_order_quantity_for_execution_venue(
        &self,
        instrument: &InstrumentAny,
        quantity: Quantity,
    ) -> Option<Quantity> {
        let quantity_decimal = Decimal::from_f64(quantity.as_f64())?;
        let normalized = provider_normalize_base_order_quantity(
            self.context.execution_venue(),
            quantity_decimal,
        )?;
        instrument
            .try_make_qty(normalized.to_f64()?, Some(true))
            .ok()
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

    /// Venue-adoption guard shared by every live event path that materializes a
    /// `Managed` position from an external event's `instrument_id` (position
    /// events via `materialize_position_from_event`, entry fills via
    /// `on_order_filled`). A live event must be on the configured execution
    /// venue; NT routes events per strategy/execution-client so a foreign-venue
    /// event should never arrive, but these paths adopt the event `instrument_id`
    /// into `Managed` with no further venue gate and the exit path then submits
    /// against it. Rather than rely only on inherited NT routing, quarantine a
    /// foreign-venue event to blind recovery and signal the caller to stop
    /// before any `Managed`/`ExitPending` transition. Returns `true` when the
    /// event was quarantined (the caller must return early), `false` when the
    /// event is on the execution venue. Mirrors the recovery-path guard in
    /// `bootstrapped_exposure_for` (single `ForeignVenuePosition` classification).
    fn quarantine_foreign_venue_event(&mut self, instrument_id: InstrumentId) -> bool {
        let execution_venue = self.context.execution_venue();
        if instrument_id.venue == execution_venue {
            return false;
        }
        log::error!(
            "binary_oracle_edge_taker live event quarantined foreign-venue instrument: strategy_id={} instrument_id={} instrument_venue={} execution_venue={}",
            self.config.strategy_id,
            instrument_id,
            instrument_id.venue,
            execution_venue,
        );
        self.exposure = ExposureState::BlindRecovery(BlindRecoveryState {
            reason: BlindRecoveryReason::ForeignVenuePosition {
                instrument_venue: instrument_id.venue,
                execution_venue,
            },
        });
        self.refresh_book_subscriptions_for_current_state();
        true
    }

    fn event_instrument_matches_held_exposure(
        &mut self,
        event_instrument_id: InstrumentId,
    ) -> bool {
        let Some(held_instrument_id) = self.exposure.held_instrument_id() else {
            return true;
        };
        if event_instrument_id == held_instrument_id {
            return true;
        }
        if self.quarantine_foreign_venue_event(event_instrument_id) {
            return false;
        }
        log::warn!(
            "binary_oracle_edge_taker ignored exposure terminal event for mismatched instrument: strategy_id={} event_instrument_id={} held_instrument_id={}",
            self.config.strategy_id,
            event_instrument_id,
            held_instrument_id,
        );
        false
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
        // Venue invariant (defense in depth): a live position event must be on the
        // execution venue, or it would be adopted into Managed and the exit path
        // would submit against a foreign instrument_id. Quarantine before any
        // Managed/ExitPending transition via the shared venue-adoption guard.
        if self.quarantine_foreign_venue_event(instrument_id) {
            return;
        }
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
        let pending_entry = pending_context
            .clone()
            .filter(|pending| self.entry_order_may_remain_working(&pending.client_order_id));
        self.exposure = match self.exposure.exit_pending().cloned() {
            Some(exit_pending)
                if exit_pending.position.as_ref().is_some_and(|managed| {
                    managed.position.position_id == position_id
                        && managed.position.instrument_id == instrument_id
                }) =>
            {
                let mut pending_exit = exit_pending.pending_exit;
                if pending_exit.fill_received {
                    // NT position updates are produced from fills; once an exit fill is known,
                    // an open position event here is authoritative residual exposure.
                    pending_exit.residual_position_observed_after_fill = true;
                }
                ExitPendingState {
                    position: Some(ManagedPositionState {
                        position: materialized_position,
                        origin,
                        pending_entry,
                    }),
                    pending_exit,
                }
                .into_state_after_exit_update()
            }
            _ => ExposureState::Managed(ManagedPositionState {
                position: materialized_position,
                origin,
                pending_entry,
            }),
        };
        self.sync_exposure_context_from_active();
        self.refresh_book_subscriptions_for_current_state();
    }

    fn mark_exit_order_terminal(
        &mut self,
        client_order_id: ClientOrderId,
        event_instrument_id: InstrumentId,
    ) {
        let Some(mut exit_pending) = self.exposure.exit_pending().cloned() else {
            return;
        };
        if exit_pending.pending_exit.client_order_id != client_order_id {
            return;
        }
        if !self.event_instrument_matches_held_exposure(event_instrument_id) {
            return;
        }
        exit_pending.pending_exit.terminal_received = true;
        self.exposure = exit_pending.into_state_after_exit_update();
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

    fn current_exit_order_for_open_position_with_config(
        &self,
        order_config: &ExitOrderExecutionConfig,
    ) -> Option<(OrderSide, f64)> {
        let open_position = &self.managed_position()?.position;
        if open_position.side != order_config.position_side {
            return None;
        }

        let order_side = order_config.side;
        let price = match order_config.order_template.order_type {
            OrderType::StopMarket | OrderType::MarketIfTouched => {
                order_config.order_template.trigger_price
            }
            OrderType::TrailingStopMarket => order_config
                .order_template
                .trigger_price
                .or(order_config.order_template.activation_price),
            _ => order_price_for_side(
                &open_position.book,
                order_side,
                order_config.order_template.is_post_only,
            ),
        }?;

        Some((order_side, price)).filter(|(_, price)| is_positive_finite(*price))
    }

    fn current_exit_value_for_open_position_with_config(
        &self,
        order_config: &ExitOrderExecutionConfig,
    ) -> Option<f64> {
        self.current_exit_order_for_open_position_with_config(order_config)
            .map(|(_, price)| price)
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
        bolt_v3_market_families::fair_probability_up_for_family(
            &self.config.rotating_market_family,
            &FairProbabilityInputs {
                spot_price,
                strike_price,
                seconds_to_market_end: seconds_to_expiry,
                realized_vol,
                pricing_kurtosis: self.config.pricing_kurtosis,
            },
        )
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
        let fair_probability_up = bolt_v3_market_families::fair_probability_up_for_family(
            &self.config.rotating_market_family,
            &FairProbabilityInputs {
                spot_price,
                strike_price,
                seconds_to_market_end: seconds_to_expiry,
                realized_vol,
                pricing_kurtosis: self.config.pricing_kurtosis,
            },
        )?;
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

    fn current_exit_ev_bps_at(
        &self,
        side: OutcomeSide,
        order_config: &ExitOrderExecutionConfig,
    ) -> Option<f64> {
        let effective_entry_cost = self.open_position_effective_entry_cost()?;
        let historical_entry_fee_bps = self.open_position_historical_entry_fee_bps()?;
        let current_exit_fee_bps = self.position_outcome_fee_bps(side)?;
        let total_entry_cost =
            effective_entry_cost * (UNIT_F64 + historical_entry_fee_bps / BPS_DENOMINATOR);
        if !is_positive_finite(total_entry_cost) {
            return None;
        }

        let current_exit_value =
            self.current_exit_value_for_open_position_with_config(order_config)?;
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

        if self
            .managed_position()
            .and_then(|managed| managed.pending_entry.as_ref())
            .is_some()
        {
            evaluation.blocked_reason = Some(EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING);
            return evaluation;
        }

        let Some(position_outcome_side) = evaluation.position_outcome_side else {
            evaluation.exit_decision = Some(ExitDecision::ExitFailClosed);
            return evaluation;
        };

        let Ok(order_config) = self.normal_exit_order_execution_config() else {
            evaluation.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_ORDER_CONFIG_INVALID);
            return evaluation;
        };
        evaluation.hold_ev_bps = self.current_hold_ev_bps_at(now_ms, position_outcome_side);
        evaluation.exit_ev_bps = self.current_exit_ev_bps_at(position_outcome_side, &order_config);
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
            order_type: None,
            order_side: None,
            position_side: None,
            time_in_force: None,
            price: None,
            quantity: None,
            client_order_id: None,
            is_post_only: None,
            is_reduce_only: None,
            is_quote_quantity: None,
            expire_time_unix_nanos: None,
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            blocked_reason: evaluation.blocked_reason,
            forced_flat_reasons: evaluation.forced_flat_reasons.clone(),
        };

        if evaluation.blocked_reason == Some(EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING) {
            return decision;
        }

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
        let Ok(order_config) =
            self.exit_order_execution_config(!evaluation.forced_flat_reasons.is_empty())
        else {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_ORDER_CONFIG_INVALID);
            return decision;
        };
        if order_config.order_template.is_quote_quantity {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_QUOTE_QUANTITY_UNSUPPORTED);
            return decision;
        }
        let Some((order_side, price)) =
            self.current_exit_order_for_open_position_with_config(&order_config)
        else {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_PRICE_MISSING);
            return decision;
        };
        if !is_positive_finite(open_position.quantity.as_f64()) {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_QUANTITY_NOT_POSITIVE);
            return decision;
        }

        decision.instrument_id = Some(open_position.instrument_id);
        decision.order_type = Some(order_config.order_template.order_type);
        decision.order_side = Some(order_side);
        decision.position_side = Some(order_config.position_side);
        decision.time_in_force = Some(order_config.order_template.time_in_force);
        decision.price = Some(price);
        decision.quantity = Some(open_position.quantity);
        decision.is_post_only = Some(order_config.order_template.is_post_only);
        decision.is_reduce_only = Some(order_config.order_template.is_reduce_only);
        decision.is_quote_quantity = Some(order_config.order_template.is_quote_quantity);
        decision.expire_time_unix_nanos = order_config.order_template.expire_time_unix_nanos;
        decision.trigger_price = order_config.order_template.trigger_price;
        decision.activation_price = order_config.order_template.activation_price;
        decision.trigger_type = order_config.order_template.trigger_type;
        decision.trigger_instrument_id = order_config.order_template.trigger_instrument_id;
        decision.trailing_offset = order_config.order_template.trailing_offset;
        decision.trailing_offset_type = order_config.order_template.trailing_offset_type;
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
        submit_context: SubmitContext,
    ) -> Result<()> {
        // A15: build the (fallible) admission request BEFORE recording the
        // order-intent evidence line. The request build can fail (e.g. a
        // market-style order whose instrument declares no structural price
        // ceiling, or an unresolvable fee bound), in which case the order never
        // fires —
        // recording the intent first would leave an orphan evidence line for an
        // order that was never submitted. Recording after the build keeps the
        // evidence chain truthful: an order-intent line exists only once the
        // order is fully valued and about to enter admission.
        let request = self.submit_admission_request_from_order(&intent, &order)?;
        self.context
            .decision_evidence()
            .record_order_intent(&intent)?;
        let permit = self.context.submit_admission().admit(&request)?;
        let result = self.submit_order(
            order,
            submit_context.position_id,
            submit_context.client_id,
            submit_context.params,
        );
        if result.is_ok() {
            permit.commit_submitted();
        }
        result
    }

    fn submit_admission_request_from_order(
        &self,
        intent: &BoltV3OrderIntentEvidence,
        order: &nautilus_model::orders::OrderAny,
    ) -> Result<BoltV3SubmitAdmissionRequest> {
        let client_order_id = order.client_order_id().to_string();
        let quantity_source = order.quantity().to_string();
        let quantity = Decimal::from_str(quantity_source.trim()).with_context(|| {
            format!(
                "bolt-v3 submit admission quantity is not a decimal for client_order_id={}",
                client_order_id
            )
        })?;
        let price_source = compiled_order_price_source(intent.price.clone(), order);
        let price = Decimal::from_str(price_source.trim()).with_context(|| {
            format!(
                "bolt-v3 submit admission price is not a decimal for client_order_id={}",
                client_order_id
            )
        })?;
        let notional = if order.is_quote_quantity() {
            let instrument = self.current_instrument(order.instrument_id()).with_context(|| {
                format!(
                    "bolt-v3 submit admission missing instrument context for quote-quantity client_order_id={}",
                    client_order_id
                )
            })?;
            let last_price = self.quote_quantity_last_price_for_order(order);
            let quote_reference_price = self.quote_quantity_reference_price_for_order(order);
            // Single source of truth: BOTH submit paths derive the admission base
            // notional from the built order through this shared helper. `None`
            // means the helper could not produce a settlement-currency notional —
            // either no reference price could be resolved, OR the instrument is
            // inverse (A6: the helper fail-closes inverse quote-quantity orders).
            // The fallback below fails CLOSED on the inverse case and otherwise
            // falls back to the raw submitted quote quantity, exactly as before.
            match admission_base_notional_from_order(
                order,
                &instrument,
                price,
                quantity,
                last_price,
                quote_reference_price,
            ) {
                Some(base_notional) => base_notional,
                None => {
                    // Degraded fallback: no reference price could be resolved.
                    // The raw submitted quote quantity equals the committed
                    // settlement-currency amount ONLY for a non-inverse
                    // instrument; for an inverse instrument the quote quantity is
                    // denominated in the quote currency, not settlement, so
                    // falling back to it would understate the cap. This system
                    // trades only non-inverse binary options — assert the
                    // invariant and FAIL CLOSED rather than admit a
                    // mis-denominated notional if that ever changes.
                    anyhow::ensure!(
                        !instrument.is_inverse(),
                        "bolt-v3 submit admission cannot value a quote-quantity order on an inverse instrument from the raw quote quantity (client_order_id={})",
                        client_order_id
                    );
                    quantity
                }
            }
        } else {
            base_quantity_admission_notional(price, quantity)
        };
        // Scale the raw notional to a fee-inclusive notional before the
        // admission cap check, using the SAME computation the proof executor
        // applies (`bolt_v3_canary_proof_executor` line 117-118). Without this
        // the strategy path would check a raw notional against a cap the proof
        // path checks fee-inclusive — admitting an order whose fee-inclusive
        // cost exceeds the intended per-order cap.
        let max_fee_bps = self.max_entry_fee_bps_for_admission(order.instrument_id(), price)?;
        // A base-quantity order with NO firm limit price (Market / StopMarket /
        // MarketIfTouched / TrailingStopMarket) carries no venue-enforced price
        // bound: it can fill, or settle, anywhere up to the instrument's
        // structural price CEILING. Value its admission notional at that ceiling —
        // the only price the venue physically cannot exceed — so the per-order cap
        // is a HARD bound on the cash such an order can commit, not an estimate. A
        // configured slippage budget is an estimate, not a bound, and must never
        // stand in for the cap.
        //
        // The ceiling is used for EVERY market-style order regardless of side or
        // intent (A4), because it is the only universally fail-closed bound:
        //   - A BUY (entry opening a long, or exit buying back a short) can debit
        //     up to qty * ceiling.
        //   - A SELL ENTRY (opening a SHORT) carries a settlement liability up to
        //     qty * ceiling — so a SELL is NOT automatically cheaper; valuing it
        //     at a price floor would UNDERSTATE a short entry's worst case and
        //     loosen the cap. The ceiling never understates any side or intent.
        // This extends the bound to market-style EXITs, which previously skipped
        // this branch and were valued at their reference/trigger price (the bug
        // A4 closed). Because an admitted entry was itself capped at the ceiling
        // and an exit never exceeds the entry quantity, a ceiling-valued exit can
        // never be spuriously blocked by the cap the entry already cleared.
        // Quote-quantity orders commit a fixed quote amount (already floored) and
        // a firm-limit order can never fill past its own price, so those keep
        // their own notional. Fail CLOSED if the instrument declares no ceiling.
        let notional = if order.price().is_none() && !order.is_quote_quantity() {
            let price_ceiling = self
                .current_instrument(order.instrument_id())
                .and_then(|instrument| instrument.max_price())
                .map(|ceiling| ceiling.as_decimal());
            market_style_admission_ceiling_notional(price_ceiling, quantity).with_context(|| {
                format!(
                    "bolt-v3 submit admission refuses a market-style order without a structural price ceiling for client_order_id={}",
                    client_order_id
                )
            })?
        } else {
            notional
        };
        let notional = fee_inclusive_admission_notional(notional, max_fee_bps);

        let intent_kind = match intent.intent_kind {
            BoltV3OrderIntentKind::Entry => BoltV3SubmitIntentKind::Entry,
            BoltV3OrderIntentKind::Exit => BoltV3SubmitIntentKind::RiskReducingExit,
        };
        let risk_reducing_exit_proof = if matches!(
            intent_kind,
            BoltV3SubmitIntentKind::RiskReducingExit
        ) {
            let managed_position = self.managed_position().ok_or_else(|| {
                anyhow::anyhow!(
                    "bolt-v3 submit admission risk-reducing exit requires managed position state for client_order_id={client_order_id}"
                )
            })?;
            let position_quantity =
                Decimal::from_f64(managed_position.position.quantity.as_f64()).with_context(
                    || {
                        format!(
                            "bolt-v3 submit admission position quantity is not a decimal for client_order_id={client_order_id}"
                        )
                    },
                )?;
            Some(BoltV3RiskReducingExitProof {
                position_id: managed_position.position.position_id.to_string(),
                instrument_id: managed_position.position.instrument_id.to_string(),
                position_side: managed_position.position.side,
                exit_order_side: order.order_side(),
                position_quantity,
                exit_quantity: quantity,
            })
        } else {
            None
        };

        Ok(BoltV3SubmitAdmissionRequest {
            strategy_id: intent.strategy_id.clone(),
            client_order_id,
            instrument_id: order.instrument_id().to_string(),
            notional,
            order_side: order.order_side(),
            order_quantity: quantity,
            intent_kind,
            lifecycle_policy: self.submit_lifecycle_policy(),
            canary_proof_claim: None,
            risk_reducing_exit_proof,
            position_sizing: self.compiled_order_sizing_evidence(order, quantity, price),
        })
    }

    fn compiled_order_sizing_evidence(
        &self,
        order: &nautilus_model::orders::OrderAny,
        quantity: Decimal,
        effective_price: Decimal,
    ) -> Option<BoltV3CompiledOrderSizingEvidence> {
        let instrument_id = order.instrument_id();
        let venue_id = instrument_id.venue.as_str().to_string();
        let side = match order.order_side() {
            OrderSide::Buy => BoltV3CompiledOrderSide::Buy,
            OrderSide::Sell => BoltV3CompiledOrderSide::Sell,
            _ => return None,
        };
        Some(BoltV3CompiledOrderSizingEvidence {
            venue_id,
            product_kind: BoltV3CompiledProductKind::PredictionMarketBinary,
            side,
            quantity,
            effective_price,
            order_kind: BoltV3CompiledOrderKind::Limit,
            liquidity: if order.is_post_only() {
                BoltV3CompiledOrderLiquidity::RestingMaker
            } else {
                BoltV3CompiledOrderLiquidity::Taker
            },
            quote_set_id: None,
            prediction_market_outcome: self.prediction_market_outcome_for_instrument(instrument_id),
        })
    }

    fn prediction_market_outcome_for_instrument(
        &self,
        instrument_id: InstrumentId,
    ) -> Option<PredictionMarketOutcomeSide> {
        prediction_market_outcome_from_fees(&self.active.outcome_fees, instrument_id)
            .or_else(|| {
                self.pending_entry().and_then(|pending| {
                    prediction_market_outcome_from_fees(&pending.outcome_fees, instrument_id)
                })
            })
            .or_else(|| {
                self.managed_position().and_then(|managed| {
                    prediction_market_outcome_from_fees(
                        &managed.position.outcome_fees,
                        instrument_id,
                    )
                })
            })
    }

    fn quote_quantity_last_price_for_order(
        &self,
        order: &nautilus_model::orders::OrderAny,
    ) -> Option<Price> {
        match order {
            OrderAny::Market(_) | OrderAny::MarketToLimit(_) => {
                self.market_order_cache_price_for_order(order)
            }
            OrderAny::StopMarket(_) | OrderAny::MarketIfTouched(_) => order.trigger_price(),
            OrderAny::TrailingStopMarket(_) | OrderAny::TrailingStopLimit(_) => {
                order.trigger_price()
            }
            _ => order.price(),
        }
    }

    fn market_order_cache_price_for_order(
        &self,
        order: &nautilus_model::orders::OrderAny,
    ) -> Option<Price> {
        let cache = self.cache();
        if let Some(last_quote) = cache.quote(&order.instrument_id()) {
            return match order.order_side() {
                OrderSide::Buy => Some(last_quote.ask_price),
                OrderSide::Sell => Some(last_quote.bid_price),
                _ => None,
            };
        }
        cache
            .trade(&order.instrument_id())
            .map(|last_trade| last_trade.price)
    }

    /// Side-appropriate top-of-book price (best ask for a BUY, best bid for a
    /// SELL) used by [`admission_base_notional_from_order`] to pick a
    /// conservative effective price for a quote-quantity order's quote→base
    /// conversion. `None` when no current quote tick is cached, which the shared
    /// helper treats as "no top-of-book" and falls back to the last price.
    fn quote_quantity_reference_price_for_order(
        &self,
        order: &nautilus_model::orders::OrderAny,
    ) -> Option<Price> {
        let cache = self.cache();
        let quote_tick = cache.quote(&order.instrument_id())?;
        match order.order_side() {
            OrderSide::Buy => Some(quote_tick.ask_price),
            OrderSide::Sell => Some(quote_tick.bid_price),
            _ => None,
        }
    }

    fn entry_strategy_input_evidence_snapshot_at(
        &self,
        now_ms: u64,
        decision: &EntrySubmissionDecision,
        client_order_id: ClientOrderId,
        price: &Price,
        quantity: &Quantity,
    ) -> Result<BoltV3StrategyInputEvidenceSnapshot> {
        let price_to_beat = self
            .active
            .price_to_beat
            .filter(|value| is_positive_finite(*value))
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires source-bound price_to_beat")
            })?;
        let interval_open = self
            .active
            .interval_open
            .filter(|value| is_positive_finite(*value))
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires positive interval_open")
            })?;
        if (interval_open - price_to_beat).abs() > f64::EPSILON {
            anyhow::bail!(
                "entry strategy input evidence requires interval_open to match source-bound price_to_beat"
            );
        }
        let reference_quote_ts_event = self.active.last_reference_ts_ms.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires reference quote timestamp")
        })?;
        let spot_price = self
            .pricing
            .spot_price()
            .filter(|value| is_positive_finite(*value))
            .ok_or_else(|| anyhow::anyhow!("entry strategy input evidence requires spot price"))?;
        let realized_volatility = self
            .current_realized_vol_at(now_ms)
            .filter(|value| is_positive_finite(*value))
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires realized volatility")
            })?;
        let seconds_to_market_end = self.current_seconds_to_expiry_at(now_ms).ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires seconds to market end")
        })?;
        let selected_side = decision.evaluation.selected_side.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires selected side")
        })?;
        let fee_rate_basis_points = self
            .entry_fee_bps_at_price(selected_side, price.as_f64())
            .filter(|value| is_non_negative_finite(*value))
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires selected outcome fee")
            })?;
        let theta_scaled_min_edge_bps = decision
            .evaluation
            .min_worst_case_ev_bps
            .filter(|value| value.is_finite())
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires theta-scaled minimum edge")
            })?;
        let fair_probability_up = decision
            .evaluation
            .fair_probability_up
            .filter(|value| value.is_finite())
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires fair probability")
            })?;
        let uncertainty_band_probability = decision
            .evaluation
            .uncertainty_band_probability
            .filter(|value| value.is_finite())
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires uncertainty band")
            })?;
        let expected_edge_basis_points = decision
            .evaluation
            .expected_ev_per_notional
            .filter(|value| value.is_finite())
            .map(|value| value * BPS_DENOMINATOR)
            .ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires expected edge")
            })?;
        let worst_case_edge_basis_points = match selected_side {
            OutcomeSide::Up => decision.evaluation.up_worst_case_ev_bps,
            OutcomeSide::Down => decision.evaluation.down_worst_case_ev_bps,
        }
        .filter(|value| value.is_finite())
        .ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires selected worst-case edge")
        })?;
        let market_start_timestamp_ms = self.active.interval_start_ms.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires market start timestamp")
        })?;
        let market_selection_timestamp_ms =
            self.active.selection_published_at_ms.ok_or_else(|| {
                anyhow::anyhow!("entry strategy input evidence requires market selection timestamp")
            })?;
        let market_end_timestamp_ms = self.active.interval_end_ms.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires market end timestamp")
        })?;
        let market_selection_outcome =
            strategy_input_market_selection_outcome(self.active.market_selection_outcome);
        let instrument_id = decision.instrument_id.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires submission instrument id")
        })?;
        let order_side = decision.order_side.ok_or_else(|| {
            anyhow::anyhow!("entry strategy input evidence requires submission order side")
        })?;
        let readiness_evidence = self.context.readiness_evidence().ok_or_else(|| {
            anyhow::anyhow!(
                "entry strategy input evidence requires readiness gate session evidence"
            )
        })?;

        Ok(BoltV3StrategyInputEvidenceSnapshot {
            strategy_id: self.config.strategy_id.clone(),
            configured_target_id: self.config.configured_target_id.clone(),
            market_selection_ruleset_id: self.config.configured_target_id.clone(),
            gate_session_hash: readiness_evidence.gate_session_hash.clone(),
            selected_market_key: readiness_evidence.selected_market_key.clone(),
            gate_evidence: readiness_evidence.gate_evidence.clone(),
            market_selection_outcome: market_selection_outcome.to_string(),
            market_id: self.active.market_id.clone(),
            polymarket_condition_id: self
                .active
                .source_identity
                .as_ref()
                .map(|identity| identity.condition_id.clone()),
            polymarket_market_slug: self
                .active
                .source_identity
                .as_ref()
                .map(|identity| identity.market_slug.clone()),
            polymarket_question_id: self
                .active
                .source_identity
                .as_ref()
                .map(|identity| identity.question_id.clone()),
            up_instrument_id: self
                .active
                .books
                .up
                .instrument_id
                .map(|instrument_id| instrument_id.to_string()),
            down_instrument_id: self
                .active
                .books
                .down
                .instrument_id
                .map(|instrument_id| instrument_id.to_string()),
            market_selection_timestamp_ms: Some(market_selection_timestamp_ms),
            selected_market_observed_timestamp_ms: Some(market_selection_timestamp_ms),
            polymarket_market_start_timestamp_ms: Some(market_start_timestamp_ms),
            polymarket_market_end_timestamp_ms: Some(market_end_timestamp_ms),
            price_to_beat_source: self.config.price_to_beat_source.clone(),
            price_to_beat_value: evidence_number(price_to_beat),
            reference_quote_ts_event,
            spot_price: evidence_number(spot_price),
            reference_fair_value: self.pricing.last_reference_fair_value.map(evidence_number),
            realized_volatility: evidence_number(realized_volatility),
            seconds_to_market_end,
            pricing_kurtosis: evidence_number(self.config.pricing_kurtosis),
            theta_decay_factor: evidence_number(self.config.theta_decay_factor),
            theta_scaled_min_edge_bps: evidence_number(theta_scaled_min_edge_bps),
            fair_probability_up: evidence_number(fair_probability_up),
            uncertainty_band_probability: evidence_number(uncertainty_band_probability),
            expected_edge_basis_points: evidence_number(expected_edge_basis_points),
            worst_case_edge_basis_points: evidence_number(worst_case_edge_basis_points),
            fee_rate_basis_points: evidence_number(fee_rate_basis_points),
            selected_side: Some(outcome_side_evidence_label(selected_side).to_string()),
            submission_instrument_id: instrument_id.to_string(),
            submission_order_side: order_side.to_string(),
            submission_price: price.to_string(),
            submission_quantity: quantity.to_string(),
            client_order_id: client_order_id.to_string(),
        })
    }

    fn submit_lifecycle_policy(&self) -> BoltV3SubmitLifecyclePolicy {
        BoltV3SubmitLifecyclePolicy::new(
            self.config.manage_contingent_orders
                || self.config.manage_gtd_expiry
                || self.config.manage_stop,
        )
    }

    fn build_configured_entry_order(
        &mut self,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
        client_order_id: ClientOrderId,
    ) -> Result<nautilus_model::orders::OrderAny> {
        anyhow::ensure!(
            !self.config.entry_order.is_reduce_only,
            "entry_is_reduce_only must be false because binary_oracle_edge_taker entry orders open the managed position"
        );
        let template = self
            .config
            .entry_order
            .nt_order_template(ORDER_CONFIGURATION_PREFIX_ENTRY, price.precision)?;
        build_nt_order(
            self.core.order_factory(),
            ORDER_CONFIGURATION_PREFIX_ENTRY,
            &template,
            NtOrderBuildInputs {
                instrument_id,
                order_side,
                quantity,
                price: Some(price),
                client_order_id,
            },
        )
    }

    fn exit_order_execution_config_from_order(
        &self,
        order: &BinaryOracleEdgeTakerOrderConfig,
        side_field: &'static str,
        position_side_field: &'static str,
    ) -> Result<ExitOrderExecutionConfig> {
        Ok(ExitOrderExecutionConfig {
            side: parse_configured_order_side(side_field, &order.side)?,
            position_side: parse_configured_position_side(
                position_side_field,
                &order.position_side,
            )?,
            order_template: ConfiguredNtOrderTemplate::from(order),
        })
    }

    fn normal_exit_order_execution_config(&self) -> Result<ExitOrderExecutionConfig> {
        self.exit_order_execution_config_from_order(
            &self.config.exit_order,
            CONFIG_FIELD_EXIT_ORDER_SIDE,
            CONFIG_FIELD_EXIT_ORDER_POSITION_SIDE,
        )
    }

    fn forced_exit_order_execution_config(&self) -> Result<ExitOrderExecutionConfig> {
        self.exit_order_execution_config_from_order(
            &self.config.forced_exit_order,
            CONFIG_FIELD_FORCED_EXIT_ORDER_SIDE,
            CONFIG_FIELD_FORCED_EXIT_ORDER_POSITION_SIDE,
        )
    }

    fn exit_order_execution_config(&self, forced_flat: bool) -> Result<ExitOrderExecutionConfig> {
        if forced_flat {
            self.forced_exit_order_execution_config()
        } else {
            self.normal_exit_order_execution_config()
        }
    }

    #[cfg(test)]
    fn build_configured_exit_order(
        &mut self,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
        client_order_id: ClientOrderId,
    ) -> Result<nautilus_model::orders::OrderAny> {
        self.build_exit_order_with_execution_config(
            self.normal_exit_order_execution_config()?,
            instrument_id,
            order_side,
            quantity,
            price,
            client_order_id,
        )
    }

    fn build_exit_order_with_execution_config(
        &mut self,
        order_config: ExitOrderExecutionConfig,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        quantity: Quantity,
        price: Price,
        client_order_id: ClientOrderId,
    ) -> Result<nautilus_model::orders::OrderAny> {
        anyhow::ensure!(
            !order_config.order_template.is_quote_quantity,
            "exit_is_quote_quantity must be false because exits are sized from base position quantity"
        );
        let template =
            order_config.nt_order_template(ORDER_CONFIGURATION_PREFIX_EXIT, price.precision)?;
        build_nt_order(
            self.core.order_factory(),
            ORDER_CONFIGURATION_PREFIX_EXIT,
            &template,
            NtOrderBuildInputs {
                instrument_id,
                order_side,
                quantity,
                price: Some(price),
                client_order_id,
            },
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
        let Some(mut quantity) = decision.quantity else {
            self.log_exit_evaluation(now_ms, &decision);
            return Ok(None);
        };
        let order_config = decision
            .execution_config()
            .ok_or_else(|| anyhow::anyhow!("exit submission decision missing order config"))?;
        let instrument = self
            .current_instrument(instrument_id)
            .ok_or_else(|| anyhow::anyhow!("exit instrument missing from cache"))?;
        let Some(normalized_quantity) =
            self.normalize_base_order_quantity_for_execution_venue(&instrument, quantity)
        else {
            decision.blocked_reason = Some(EXIT_BLOCK_REASON_EXIT_QUANTITY_NOT_POSITIVE);
            self.log_exit_evaluation(now_ms, &decision);
            return Ok(None);
        };
        quantity = normalized_quantity;
        decision.quantity = Some(quantity);
        let price = Price::new(raw_price, instrument.price_precision());
        let client_order_id = self.core.order_factory().generate_client_order_id();
        decision.client_order_id = Some(client_order_id);
        self.log_exit_evaluation(now_ms, &decision);
        let order = self.build_exit_order_with_execution_config(
            order_config,
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
        if !decision.forced_flat_reasons.is_empty()
            && let Some(pending_entry) = managed_position.pending_entry.as_ref()
        {
            self.cancel_order(pending_entry.client_order_id, Some(client_id), None)
                .with_context(|| {
                    format!(
                        "forced-flat exit could not cancel pending entry client_order_id={}",
                        pending_entry.client_order_id
                    )
                })?;
        }
        self.exposure = ExposureState::ExitPending(ExitPendingState {
            position: Some(managed_position.clone()),
            pending_exit: PendingExitState {
                client_order_id,
                market_id: managed_position.position.market_id.clone(),
                position_id: Some(managed_position.position.position_id),
                fill_received: false,
                close_received: false,
                terminal_received: false,
                residual_position_observed_after_fill: false,
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

        let intent = BoltV3OrderIntentEvidence::from_compiled_order(
            self.config.strategy_id.clone(),
            BoltV3OrderIntentKind::Exit,
            price.to_string(),
            &order,
        );

        if let Err(error) = self.submit_order_with_decision_evidence(
            intent,
            order,
            SubmitContext::with_client_id_and_position_id(
                client_id,
                managed_position.position.position_id,
            ),
        ) {
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
        let shares_value = if self.config.entry_order.is_quote_quantity {
            sized_notional
        } else {
            sized_notional / entry_cost
        };
        let Ok(quantity) = instrument.try_make_qty(shares_value, Some(true)) else {
            decision.blocked_reason = Some(ENTRY_BLOCK_REASON_QUANTITY_ROUNDING_FAILED);
            return decision;
        };
        let Some(quantity) =
            self.normalize_base_order_quantity_for_execution_venue(&instrument, quantity)
        else {
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
            .and_then(|selected_side| self.entry_fee_bps_at_price(selected_side, price))
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
        let strategy_input_snapshot = self.entry_strategy_input_evidence_snapshot_at(
            now_ms,
            &decision,
            client_order_id,
            &price,
            &quantity,
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

        let intent = BoltV3OrderIntentEvidence::from_compiled_order(
            self.config.strategy_id.clone(),
            BoltV3OrderIntentKind::Entry,
            price.to_string(),
            &order,
        );

        if let Err(error) = self
            .context
            .decision_evidence()
            .record_strategy_input_snapshot(&strategy_input_snapshot)
            .and_then(|()| {
                self.submit_order_with_decision_evidence(
                    intent,
                    order,
                    SubmitContext::with_client_id(client_id),
                )
            })
        {
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
        let up_fee_bps = match self.entry_fee_bps_at_price(OutcomeSide::Up, up_entry_cost) {
            Some(value) => value,
            None => {
                evaluation
                    .pricing_blocked_by
                    .push(EntryPricingBlockReason::FeeUnavailable(OutcomeSide::Up));
                return evaluation;
            }
        };
        let down_fee_bps = match self.entry_fee_bps_at_price(OutcomeSide::Down, down_entry_cost) {
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
        if self
            .reference_instrument_id()
            .is_none_or(|instrument_id| quote.instrument_id != instrument_id)
        {
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

        self.refresh_fee_readiness();
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

    fn on_trade(&mut self, trade: &TradeTick) -> anyhow::Result<()> {
        if let Some(trade_flow) = self.active.trade_flow.get_mut(&trade.instrument_id) {
            trade_flow.observe(trade);
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
        let managed_entry_fill = self.managed_position().is_some_and(|managed| {
            managed
                .pending_entry
                .as_ref()
                .is_some_and(|pending| pending.client_order_id == event.client_order_id)
        });
        let exit_fill = self
            .exposure
            .exit_pending()
            .is_some_and(|exit| exit.pending_exit.client_order_id == event.client_order_id);

        if entry_fill {
            let pending_context = self.pending_entry_context_for(event.instrument_id);
            let keep_pending_entry = self.entry_order_may_remain_working(&event.client_order_id);
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
            if managed_entry_fill {
                if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                    return Ok(());
                }
                if let Some(exit_pending) = self.exposure.exit_pending_mut() {
                    exit_pending
                        .pending_exit
                        .residual_position_observed_after_fill = true;
                }
                if !keep_pending_entry {
                    self.clear_managed_pending_entry_for_client_order(
                        event.client_order_id,
                        event.instrument_id,
                    );
                }
            } else if let (Some(position_id), Some(position_side)) =
                (event.position_id, position_side)
            {
                // Venue invariant (shared guard): never adopt a foreign-venue fill
                // into Managed — the exit path would submit against it. Same
                // venue-adoption class as the position-event path above.
                if self.quarantine_foreign_venue_event(event.instrument_id) {
                    return Ok(());
                }
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
                    pending_entry: pending_context.clone().filter(|_| keep_pending_entry),
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
            if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                return Ok(());
            }
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
        self.clear_pending_entry_for_client_order(event.client_order_id, event.instrument_id);
        self.mark_exit_order_terminal(event.client_order_id, event.instrument_id);
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
        Ok(())
    }
}

nautilus_strategy!(BinaryOracleEdgeTaker, {
    fn on_order_rejected(&mut self, event: nautilus_model::events::OrderRejected) {
        self.clear_pending_entry_for_client_order(event.client_order_id, event.instrument_id);
        self.mark_exit_order_terminal(event.client_order_id, event.instrument_id);
        self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
    }

    fn on_order_expired(&mut self, event: nautilus_model::events::OrderExpired) {
        self.clear_pending_entry_for_client_order(event.client_order_id, event.instrument_id);
        self.mark_exit_order_terminal(event.client_order_id, event.instrument_id);
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
        let managed_position_close = match &self.exposure {
            ExposureState::Managed(position)
                if position.position.position_id == event.position_id =>
            {
                Some(position.pending_entry.clone())
            }
            _ => None,
        };
        if let Some(pending_entry) = managed_position_close {
            if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                return;
            }
            if let Some(pending_entry) = pending_entry {
                let client_order_id = pending_entry.client_order_id;
                self.exposure = ExposureState::PendingEntry(pending_entry);
                let client_id = ClientId::from(self.config.client_id.as_str());
                if let Err(error) = self.cancel_order(client_order_id, Some(client_id), None) {
                    log::error!(
                        "binary_oracle_edge_taker external position close could not cancel pending entry: strategy_id={} client_order_id={} error={error}",
                        self.config.strategy_id,
                        client_order_id,
                    );
                }
            } else {
                self.exposure = ExposureState::Flat;
            }
            self.refresh_book_subscriptions_for_current_state();
            self.prune_market_lifecycle(event.ts_event.as_u64() / NANOS_PER_MILLI_U64);
            return;
        }

        let exit_pending_close = self.exposure.exit_pending().is_some_and(|exit_pending| {
            exit_pending.pending_exit.position_id == Some(event.position_id)
        });
        if exit_pending_close {
            if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                return;
            }
            if let ExposureState::ExitPending(exit_pending) = &mut self.exposure {
                exit_pending.pending_exit.close_received = true;
                exit_pending.position = None;
                if exit_pending.is_terminal() {
                    self.exposure = ExposureState::Flat;
                }
            }
        } else if matches!(
            &self.exposure,
            ExposureState::UnsupportedObserved(observed)
                if observed.observed.position_id == event.position_id
        ) {
            if !self.event_instrument_matches_held_exposure(event.instrument_id) {
                return;
            }
            if matches!(
                &self.exposure,
                ExposureState::UnsupportedObserved(observed)
                    if observed.observed.position_id == event.position_id
            ) {
                self.exposure = ExposureState::Flat;
            }
        } else {
            // Entry reconciliation may not have a position id yet; the instrument is the
            // strongest available key for a close that races ahead of position materialization.
            if matches!(
                &self.exposure,
                ExposureState::EntryReconcilePending { pending, .. }
                    if pending.instrument_id == event.instrument_id
            ) {
                self.exposure = ExposureState::Flat;
            }
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
const FORCED_EXIT_ORDER_FIELD: &str = stringify!(forced_exit_order);
const WRONG_TYPE_CODE: &str = stringify!(wrong_type);
const UNKNOWN_FIELD_CODE: &str = stringify!(unknown_field);
const TARGET_MARKET_NOT_FOUND_REASON: &str = stringify!(target_market_not_found);

impl BinaryOracleEdgeTakerBuilder {
    fn parse_config(raw: &Value) -> Result<BinaryOracleEdgeTakerConfig> {
        let config: BinaryOracleEdgeTakerConfig = raw
            .clone()
            .try_into()
            .context("binary_oracle_edge_taker builder requires a valid config table")?;
        // Fail loud at load: a non-positive spike_guard_return_threshold makes the
        // spike guard's `relative_move >= threshold` test (relative_move is an
        // abs(), always >= 0) always true, arming the cooldown on every reference
        // quote and silently blocking all entry. The TOML type check accepts
        // 0.0/negatives, so the range is validated here, matching the build-path
        // `is_positive_finite` precedent (price_from_config / trailing_offset_from_config).
        anyhow::ensure!(
            is_positive_finite(config.spike_guard_return_threshold),
            "spike_guard_return_threshold must be positive and finite"
        );
        // Fail loud at load: a zero trade-flow sample cap makes the count-cap
        // evict every observation, permanently emptying the buffer and starving
        // the W3 read seam. The TOML type check accepts 0, so the bound is checked
        // here.
        anyhow::ensure!(
            config.trade_flow_max_samples > 0,
            "trade_flow_max_samples must be positive"
        );
        Ok(config)
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
                    | FORCED_EXIT_ORDER_FIELD
                    | "reference_venue"
                    | "reference_instrument_id"
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
        Self::validate_optional_string_field(table, field_prefix, "reference_venue", errors);
        Self::validate_optional_string_field(
            table,
            field_prefix,
            "reference_instrument_id",
            errors,
        );
        if table.contains_key("reference_venue") != table.contains_key("reference_instrument_id") {
            let missing = if table.contains_key("reference_venue") {
                "reference_instrument_id"
            } else {
                "reference_venue"
            };
            Self::push_missing(
                errors,
                format!("{field_prefix}.{missing}"),
                "missing_reference_data_pair",
                BinaryOracleEdgeTakerFieldType::String,
            );
        }
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
        Self::validate_order_table(
            table,
            field_prefix,
            FORCED_EXIT_ORDER_FIELD,
            concat!(stringify!(missing_), stringify!(forced_exit_order)),
            errors,
        );
        Self::validate_rotating_market_family(table, field_prefix, errors);
    }

    /// Reject an unknown `rotating_market_family` at config-parse time (P5-10).
    /// Startup market-identity construction already fails loud on an unknown
    /// family, so this is defense-in-depth that converges parse-time validation
    /// with the SINGLE registry source of truth
    /// (`bolt_v3_market_families::validation_bindings`): a family the registry
    /// does not bind can never be selected or traded, so it must be rejected here
    /// rather than accepted and only caught later.
    fn validate_rotating_market_family(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        errors: &mut Vec<ValidationError>,
    ) {
        let field_name = stringify!(rotating_market_family);
        let Some(value) = table.get(field_name) else {
            // Presence/type is enforced by the generated field validator; an
            // absent or non-string value is reported there, not here.
            return;
        };
        let Some(family) = value.as_str() else {
            return;
        };
        let is_known = bolt_v3_market_families::validation_bindings()
            .iter()
            .any(|binding| binding.key == family);
        if !is_known {
            errors.push(ValidationError {
                field: format!("{field_prefix}.{field_name}"),
                code: stringify!(unknown_market_family),
                message: format!("unknown market family `{family}`"),
            });
        }
    }

    fn validate_optional_string_field(
        table: &toml::map::Map<String, Value>,
        field_prefix: &str,
        field_name: &'static str,
        errors: &mut Vec<ValidationError>,
    ) {
        if let Some(value) = table.get(field_name)
            && !BinaryOracleEdgeTakerFieldType::String.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field_prefix}.{field_name}"),
                BinaryOracleEdgeTakerFieldType::String,
                value,
            );
        }
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
                ORDER_EXPIRE_TIME_UNIX_NANOS_FIELD
                    | ORDER_TRIGGER_PRICE_FIELD
                    | ORDER_ACTIVATION_PRICE_FIELD
                    | ORDER_TRIGGER_TYPE_FIELD
                    | ORDER_TRIGGER_INSTRUMENT_ID_FIELD
                    | ORDER_TRAILING_OFFSET_FIELD
                    | ORDER_TRAILING_OFFSET_TYPE_FIELD
                    | binary_oracle_edge_taker_order_fields!(match_order_field_names)
            ) {
                Self::push_unknown_field(errors, format!("{field}.{key}"), key);
            }
        }

        binary_oracle_edge_taker_order_fields!(validate_order_fields_impl)(
            order_table,
            &field,
            errors,
        );
        if let Some(value) = order_table.get(ORDER_EXPIRE_TIME_UNIX_NANOS_FIELD)
            && !BinaryOracleEdgeTakerFieldType::Integer.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_EXPIRE_TIME_UNIX_NANOS_FIELD}"),
                BinaryOracleEdgeTakerFieldType::Integer,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_TRIGGER_PRICE_FIELD)
            && !BinaryOracleEdgeTakerFieldType::Float.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_TRIGGER_PRICE_FIELD}"),
                BinaryOracleEdgeTakerFieldType::Float,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_ACTIVATION_PRICE_FIELD)
            && !BinaryOracleEdgeTakerFieldType::Float.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_ACTIVATION_PRICE_FIELD}"),
                BinaryOracleEdgeTakerFieldType::Float,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_TRIGGER_TYPE_FIELD)
            && !BinaryOracleEdgeTakerFieldType::String.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_TRIGGER_TYPE_FIELD}"),
                BinaryOracleEdgeTakerFieldType::String,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_TRIGGER_INSTRUMENT_ID_FIELD)
            && !BinaryOracleEdgeTakerFieldType::String.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_TRIGGER_INSTRUMENT_ID_FIELD}"),
                BinaryOracleEdgeTakerFieldType::String,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_TRAILING_OFFSET_FIELD)
            && !BinaryOracleEdgeTakerFieldType::Float.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_TRAILING_OFFSET_FIELD}"),
                BinaryOracleEdgeTakerFieldType::Float,
                value,
            );
        }
        if let Some(value) = order_table.get(ORDER_TRAILING_OFFSET_TYPE_FIELD)
            && !BinaryOracleEdgeTakerFieldType::String.matches(value)
        {
            Self::push_wrong_type(
                errors,
                format!("{field}.{ORDER_TRAILING_OFFSET_TYPE_FIELD}"),
                BinaryOracleEdgeTakerFieldType::String,
                value,
            );
        }
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

pub const ENTRY_DECISION_EVIDENCE_SOURCE_SCHEMA_VERSION: u32 = 2;
pub const ENTRY_DECISION_EVIDENCE_SOURCE_RECORD_KIND: &str =
    "bolt_v3.binary_oracle_entry_decision_source.v2";
const ENTRY_DECISION_PRICE_TO_BEAT_VALUE_FIELD: &str = "price_to_beat_value";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryDecisionEvidenceSource {
    pub schema_version: u32,
    pub record_kind: String,
    pub market_selection_timestamp_ms: u64,
    pub decision_timestamp_ms: u64,
    pub readiness_session: EntryReadinessGateSession,
    pub warmup_count: u64,
    pub reference_quote: BinaryOracleEntryReferenceQuoteSource,
    pub realized_volatility: BinaryOracleEntryRealizedVolatilitySource,
    pub fees: BinaryOracleEntryFeeSource,
    pub books: BinaryOracleEntryBooksSource,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryReferenceQuoteSource {
    pub venue: String,
    pub price: f64,
    pub observed_ts_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryRealizedVolatilitySource {
    pub value: f64,
    pub ready_ts_ms: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct BinaryOracleReferenceQuoteObservationSource<'a> {
    pub data_client_id: &'a str,
    pub instrument_id: &'a str,
    pub bid_price: f64,
    pub ask_price: f64,
    pub ts_event_unix_nanos: u64,
}

#[derive(Debug, Clone)]
pub struct BinaryOracleEntryReferenceProofSources {
    pub reference_quote: BinaryOracleEntryReferenceQuoteSource,
    pub realized_volatility: BinaryOracleEntryRealizedVolatilitySource,
}

pub fn derive_entry_reference_proofs_from_quote_observations(
    raw_config: &Value,
    observations: &[BinaryOracleReferenceQuoteObservationSource<'_>],
    market_selection_timestamp_ms: u64,
    decision_timestamp_ms: u64,
) -> Result<BinaryOracleEntryReferenceProofSources> {
    if decision_timestamp_ms < market_selection_timestamp_ms {
        anyhow::bail!("entry reference proof decision timestamp precedes market selection");
    }

    let config = BinaryOracleEdgeTakerBuilder::parse_config(raw_config)?;
    let reference_venue = config.reference_venue.as_ref().ok_or_else(|| {
        anyhow::anyhow!("reference quote observation source requires configured reference_venue")
    })?;
    let reference_instrument_id = config.reference_instrument_id.as_ref().ok_or_else(|| {
        anyhow::anyhow!(
            "reference quote observation source requires configured reference_instrument_id"
        )
    })?;
    let mut sorted_observations = observations.to_vec();
    sorted_observations.sort_by_key(|observation| observation.ts_event_unix_nanos);
    let mut estimator = RealizedVolEstimator::from_config(&realized_vol_config(&config));
    let mut latest_quote = None;
    let mut latest_ready_volatility = None;

    for observation in sorted_observations {
        if observation.data_client_id != *reference_venue
            || observation.instrument_id != *reference_instrument_id
        {
            continue;
        }
        if observation.ts_event_unix_nanos == 0
            || !is_positive_finite(observation.bid_price)
            || !is_positive_finite(observation.ask_price)
        {
            anyhow::bail!("reference quote observation source contains invalid quote data");
        }
        let observed_ts_ms = observation.ts_event_unix_nanos / NANOS_PER_MILLI_U64;
        if observed_ts_ms > decision_timestamp_ms {
            continue;
        }
        let midpoint = (observation.bid_price + observation.ask_price) / MIDPOINT_DIVISOR_F64;
        if !is_positive_finite(midpoint) {
            anyhow::bail!("reference quote observation source midpoint is invalid");
        }
        let quote = FastSpotObservation {
            venue_name: reference_venue.clone(),
            price: midpoint,
            observed_ts_ms,
        };
        if let Some(value) = estimator.observe(&quote.venue_name, quote.price, quote.observed_ts_ms)
            && observed_ts_ms >= market_selection_timestamp_ms
        {
            latest_ready_volatility = Some(BinaryOracleEntryRealizedVolatilitySource {
                value,
                ready_ts_ms: observed_ts_ms,
            });
        }
        if observed_ts_ms >= market_selection_timestamp_ms {
            latest_quote = Some(BinaryOracleEntryReferenceQuoteSource {
                venue: reference_venue.clone(),
                price: midpoint,
                observed_ts_ms,
            });
        }
    }

    let reference_quote = latest_quote.ok_or_else(|| {
        anyhow::anyhow!(
            "reference quote observation source did not produce a configured reference quote"
        )
    })?;
    let realized_volatility = latest_ready_volatility.ok_or_else(|| {
        anyhow::anyhow!(
            "reference quote observation source did not produce ready realized volatility"
        )
    })?;
    Ok(BinaryOracleEntryReferenceProofSources {
        reference_quote,
        realized_volatility,
    })
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryFeeSource {
    pub fee_bps_by_instrument_id: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryBooksSource {
    pub price_precision: u8,
    pub up: BinaryOracleEntryBookSideSource,
    pub down: BinaryOracleEntryBookSideSource,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleEntryBookSideSource {
    pub best_bid: f64,
    pub bid_quantity: f64,
    pub best_ask: f64,
    pub ask_quantity: f64,
    pub liquidity_available: f64,
}

#[derive(Debug, Clone)]
struct SourceFeeProvider {
    fee_bps_by_instrument_id: BTreeMap<String, Decimal>,
}

impl FeeProvider for SourceFeeProvider {
    fn fee_bps(&self, instrument_id: InstrumentId) -> Option<Decimal> {
        self.fee_bps_by_instrument_id
            .get(&instrument_id.to_string())
            .copied()
    }

    fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
        async { Ok(()) }.boxed()
    }
}

pub fn record_entry_decision_evidence_from_source(
    raw_config: &Value,
    decision_evidence: Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
    trader_id: TraderId,
    source: &BinaryOracleEntryDecisionEvidenceSource,
    instruments: &[InstrumentAny],
) -> Result<()> {
    validate_entry_decision_source(source)?;
    if instruments.is_empty() {
        anyhow::bail!("entry decision evidence source requires at least one instrument");
    }

    let fee_provider = Arc::new(SourceFeeProvider {
        fee_bps_by_instrument_id: source_fee_bps_by_instrument_id(source)?,
    });
    let submit_admission = Arc::new(
        crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(
            decision_evidence.clone(),
        ),
    );
    let readiness_evidence = BoltV3ReadinessGateEvidenceSnapshot::from_entry_readiness_gate_session(
        &source.readiness_session,
    );
    let price_to_beat =
        entry_decision_price_to_beat_from_readiness_session(&source.readiness_session)?;
    // Execution venue for this replay is the venue of the stored selection's outcome instruments
    // (the strategy only ever trades the venue its selected market is on). This offline evidence
    // path validates against that stored selection and never reads the live cache, so the venue is
    // used only for context completeness; it is still derived from the source rather than assumed.
    let execution_venue = source
        .readiness_session
        .selected_market
        .instrument_ids
        .iter()
        .find_map(|instrument_id| InstrumentId::from_str(instrument_id.as_str()).ok())
        .map(|instrument_id| instrument_id.venue)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "entry decision evidence source selected market is missing a parseable instrument required for execution-venue resolution"
            )
        })?;
    let context = StrategyBuildContext::new(
        fee_provider,
        decision_evidence,
        submit_admission,
        execution_venue,
    )
    .with_readiness_evidence(readiness_evidence);
    let mut strategy = BinaryOracleEdgeTaker::new(
        BinaryOracleEdgeTakerBuilder::parse_config(raw_config)?,
        context,
    );
    register_source_replay_strategy(&mut strategy, trader_id, source, instruments)?;

    let mut selection =
        selection_snapshot_from_entry_decision_source(&strategy.config, source, instruments);
    let SelectionState::Active { market } = &mut selection.decision.state else {
        anyhow::bail!("entry decision evidence source did not select an active configured market");
    };
    market.price_to_beat = Some(price_to_beat);
    selection.published_at_ms = source.market_selection_timestamp_ms;
    strategy.apply_selection_snapshot(selection);
    strategy.observe_reference_quote(&FastSpotObservation {
        venue_name: source.reference_quote.venue.clone(),
        price: source.reference_quote.price,
        observed_ts_ms: source.reference_quote.observed_ts_ms,
    });
    strategy.active.warmup_count = source.warmup_count;
    strategy.pricing.realized_vol.last_ready_vol = Some(source.realized_volatility.value);
    strategy.pricing.realized_vol.last_ready_ts_ms = Some(source.realized_volatility.ready_ts_ms);
    strategy.refresh_fee_readiness();
    apply_entry_decision_source_books(&mut strategy, &source.books)?;

    match strategy.try_submit_entry_order(source.decision_timestamp_ms) {
        Err(error) => Err(error),
        Ok(Some(_client_order_id)) => Ok(()),
        Ok(None) => {
            let decision = strategy.entry_submission_decision_at(source.decision_timestamp_ms);
            anyhow::bail!(
                "entry decision evidence source did not produce an entry order: blocked_reason={:?} gate_blocked_by={:?} pricing_blocked_by={:?} selected_side={:?} up_worst_case_ev_bps={:?} down_worst_case_ev_bps={:?} min_worst_case_ev_bps={:?} sized_notional={:?}",
                decision.blocked_reason,
                decision.evaluation.gate.blocked_by,
                decision.evaluation.pricing_blocked_by,
                decision.evaluation.selected_side,
                decision.evaluation.up_worst_case_ev_bps,
                decision.evaluation.down_worst_case_ev_bps,
                decision.evaluation.min_worst_case_ev_bps,
                decision.evaluation.sized_notional,
            )
        }
    }
}

fn validate_entry_decision_source(source: &BinaryOracleEntryDecisionEvidenceSource) -> Result<()> {
    anyhow::ensure!(
        source.schema_version == ENTRY_DECISION_EVIDENCE_SOURCE_SCHEMA_VERSION,
        "entry decision evidence source schema_version is invalid"
    );
    anyhow::ensure!(
        source.record_kind == ENTRY_DECISION_EVIDENCE_SOURCE_RECORD_KIND,
        "entry decision evidence source record_kind is invalid"
    );
    anyhow::ensure!(
        source.decision_timestamp_ms >= source.market_selection_timestamp_ms,
        "entry decision evidence source decision_timestamp_ms precedes market selection"
    );
    let readiness_evidence = BoltV3ReadinessGateEvidenceSnapshot::from_entry_readiness_gate_session(
        &source.readiness_session,
    );
    validate_readiness_gate_evidence_snapshot(&readiness_evidence)?;
    entry_decision_price_to_beat_from_readiness_session(&source.readiness_session)?;
    anyhow::ensure!(
        is_positive_finite(source.reference_quote.price),
        "entry decision evidence source reference quote price is invalid"
    );
    anyhow::ensure!(
        is_positive_finite(source.realized_volatility.value),
        "entry decision evidence source realized volatility is invalid"
    );
    Ok(())
}

fn entry_decision_price_to_beat_from_readiness_session(
    session: &EntryReadinessGateSession,
) -> Result<f64> {
    let satisfaction = session
        .satisfied_roles
        .get(RESOLUTION_GATE_ROLE)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "entry decision evidence source readiness_session is missing resolution evidence"
            )
        })?;
    let GateSatisfaction::Evidence { evidence } = satisfaction else {
        anyhow::bail!(
            "entry decision evidence source readiness_session resolution evidence is required"
        );
    };
    anyhow::ensure!(
        evidence.value_kind == PRICE_GATE_VALUE_KIND,
        "entry decision evidence source readiness_session resolution value_kind is invalid"
    );
    let value = evidence
        .normalized_value
        .get(ENTRY_DECISION_PRICE_TO_BEAT_VALUE_FIELD)
        .and_then(json_value_as_f64)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "entry decision evidence source readiness_session price_to_beat_value is invalid"
            )
        })?;
    anyhow::ensure!(
        is_positive_finite(value),
        "entry decision evidence source readiness_session price_to_beat_value is invalid"
    );
    Ok(value)
}

fn json_value_as_f64(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|value| value as f64))
        .or_else(|| value.as_u64().map(|value| value as f64))
}

fn source_fee_bps_by_instrument_id(
    source: &BinaryOracleEntryDecisionEvidenceSource,
) -> Result<BTreeMap<String, Decimal>> {
    let mut fees = BTreeMap::new();
    for (instrument_id, fee_bps) in &source.fees.fee_bps_by_instrument_id {
        anyhow::ensure!(
            !instrument_id.trim().is_empty(),
            "entry decision evidence source fee instrument id is required"
        );
        anyhow::ensure!(
            is_non_negative_finite(*fee_bps),
            "entry decision evidence source fee bps is invalid"
        );
        let fee_bps = Decimal::from_f64(*fee_bps)
            .ok_or_else(|| anyhow::anyhow!("entry decision evidence source fee bps is invalid"))?;
        fees.insert(instrument_id.clone(), fee_bps);
    }
    Ok(fees)
}

fn register_source_replay_strategy(
    strategy: &mut BinaryOracleEdgeTaker,
    trader_id: TraderId,
    source: &BinaryOracleEntryDecisionEvidenceSource,
    instruments: &[InstrumentAny],
) -> Result<()> {
    let clock = Rc::new(RefCell::new(TestClock::new()));
    clock.borrow_mut().set_time(UnixNanos::from(
        source
            .decision_timestamp_ms
            .saturating_mul(NANOS_PER_MILLI_U64),
    ));
    let cache = Rc::new(RefCell::new(Cache::new(None, None)));
    let portfolio = Rc::new(RefCell::new(Portfolio::new(
        cache.clone(),
        clock.clone(),
        None,
    )));
    strategy
        .core
        .register(trader_id, clock, cache.clone(), portfolio)
        .context("failed to register source replay strategy core")?;
    let mut cache = cache.borrow_mut();
    for instrument in instruments {
        cache
            .add_instrument(instrument.clone())
            .context("failed to add source replay instrument to cache")?;
    }
    Ok(())
}

fn apply_entry_decision_source_books(
    strategy: &mut BinaryOracleEdgeTaker,
    books: &BinaryOracleEntryBooksSource,
) -> Result<()> {
    apply_entry_decision_source_book(
        &mut strategy.active.books.up,
        &books.up,
        books.price_precision,
    )
    .context("entry decision evidence up book source is invalid")?;
    apply_entry_decision_source_book(
        &mut strategy.active.books.down,
        &books.down,
        books.price_precision,
    )
    .context("entry decision evidence down book source is invalid")?;
    Ok(())
}

fn apply_entry_decision_source_book(
    book: &mut OutcomeBookState,
    source: &BinaryOracleEntryBookSideSource,
    price_precision: u8,
) -> Result<()> {
    let instrument_id = book
        .instrument_id
        .ok_or_else(|| anyhow::anyhow!("entry decision evidence book is missing instrument id"))?;
    anyhow::ensure!(
        is_positive_finite(source.best_bid)
            && is_positive_finite(source.best_ask)
            && is_positive_finite(source.bid_quantity)
            && is_positive_finite(source.ask_quantity)
            && is_positive_finite(source.liquidity_available),
        "entry decision evidence book contains non-positive values"
    );
    anyhow::ensure!(
        source.best_bid <= source.best_ask,
        "entry decision evidence book best_bid exceeds best_ask"
    );
    book.last_observed_instrument_id = Some(instrument_id);
    book.bid_levels.clear();
    book.ask_levels.clear();
    let best_bid = Price::new_checked(source.best_bid, price_precision).map_err(|source| {
        anyhow::anyhow!("entry decision evidence book bid is invalid: {source}")
    })?;
    let best_ask = Price::new_checked(source.best_ask, price_precision).map_err(|source| {
        anyhow::anyhow!("entry decision evidence book ask is invalid: {source}")
    })?;
    book.bid_levels.insert(best_bid, source.bid_quantity);
    book.ask_levels.insert(best_ask, source.ask_quantity);
    book.best_bid = Some(source.best_bid);
    book.best_ask = Some(source.best_ask);
    book.liquidity_available = Some(source.liquidity_available);
    Ok(())
}

fn apply_selection_snapshot_to_active(
    active: &mut ActiveMarketState,
    snapshot: &RuntimeSelectionSnapshot,
    warmup_target: u64,
) {
    let previous_books = active.books.clone();
    let previous_trade_flow = std::mem::take(&mut active.trade_flow);
    let next = ActiveMarketState::from_snapshot(snapshot, warmup_target);
    let preserve_books = active.market_id.is_some()
        && active.market_id == next.market_id
        && active.instrument_id == next.instrument_id;
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

fn same_market_transition(current: &ActiveMarketState, next: &ActiveMarketState) -> bool {
    current.market_id.is_some()
        && current.market_id == next.market_id
        && current.instrument_id == next.instrument_id
        && current.market_selection_outcome == next.market_selection_outcome
        && current.interval_start_ms == next.interval_start_ms
        && current.interval_end_ms == next.interval_end_ms
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

/// True unless `snapshot` selects an Active (or, in tests, Freeze) market whose up or down outcome
/// instrument is on a venue other than `execution_venue`. An Idle snapshot has no selected market to
/// route a real order to and trivially matches. An outcome instrument id that cannot be parsed fails
/// closed (treated as NOT on the execution venue), so a malformed selection can never pass the gate.
fn selected_market_on_execution_venue(
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

fn outcome_on_execution_venue(outcome: &CandidateOutcome, execution_venue: Venue) -> bool {
    InstrumentId::from_str(&outcome.instrument_id)
        .map(|instrument_id| instrument_id.venue == execution_venue)
        .unwrap_or(false)
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

fn selection_snapshot_from_entry_decision_source(
    config: &BinaryOracleEdgeTakerConfig,
    source: &BinaryOracleEntryDecisionEvidenceSource,
    instruments: &[InstrumentAny],
) -> RuntimeSelectionSnapshot {
    let Some(market) = selected_source_market_from_instruments(config, source, instruments) else {
        return idle_selection_snapshot(
            config,
            source.market_selection_timestamp_ms,
            TARGET_MARKET_NOT_FOUND_REASON,
        );
    };
    selection_snapshot_for_state(
        config,
        source.market_selection_timestamp_ms,
        SelectionState::Active { market },
    )
}

fn selected_source_market_from_instruments(
    config: &BinaryOracleEdgeTakerConfig,
    source: &BinaryOracleEntryDecisionEvidenceSource,
    instruments: &[InstrumentAny],
) -> Option<CandidateMarket> {
    let selected = &source.readiness_session.selected_market;
    let expected_instrument_ids = selected
        .instrument_ids
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    if expected_instrument_ids.len() != selected.instrument_ids.len() {
        return None;
    }
    let cadence_milliseconds = config.cadence_seconds.checked_mul(MILLIS_PER_SECOND_U64)?;
    let max_attempts = instruments.len().max(COUNTER_INCREMENT);
    for attempt_index in INITIAL_COUNTER_USIZE..max_attempts {
        let attempt_offset =
            cadence_milliseconds.checked_mul(u64::try_from(attempt_index).ok()?)?;
        let attempt_now_ms = source
            .market_selection_timestamp_ms
            .checked_add(attempt_offset)?;
        let Some(market) =
            select_configured_market_from_instruments(config, instruments, attempt_now_ms)
        else {
            continue;
        };
        if market.market_id != selected.market_id {
            continue;
        }
        let market_instrument_ids = BTreeSet::from([
            market.up.instrument_id.clone(),
            market.down.instrument_id.clone(),
        ]);
        if market_instrument_ids == expected_instrument_ids {
            return Some(market);
        }
    }
    None
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
        source_identity: market.source_identity,
        selection_outcome: market.selection_outcome,
        price_to_beat: None,
        start_ts_ms: market.start_timestamp_milliseconds,
        expiration_ts_ms: market.expiration_timestamp_milliseconds,
        seconds_to_end: market.seconds_to_end,
    })
}

fn strategy_input_market_selection_outcome(outcome: MarketSelectionOutcome) -> &'static str {
    match outcome {
        MarketSelectionOutcome::Current => BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_CURRENT,
        MarketSelectionOutcome::Next => BOLT_V3_STRATEGY_INPUT_MARKET_SELECTION_OUTCOME_NEXT,
    }
}

fn prediction_market_outcome_from_fees(
    fees: &OutcomeFeeState,
    instrument_id: InstrumentId,
) -> Option<PredictionMarketOutcomeSide> {
    if fees.up_instrument_id == Some(instrument_id) {
        return Some(PredictionMarketOutcomeSide::Yes);
    }
    if fees.down_instrument_id == Some(instrument_id) {
        return Some(PredictionMarketOutcomeSide::No);
    }
    None
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
        #[cfg(not(test))]
        strategy.unsubscribe_trades(instrument_id, None, None);
        strategy.active.trade_flow.remove(&instrument_id);
        strategy.record_book_subscription_event(BookSubscriptionEvent::unsubscribe(instrument_id));
    }
    if let Some(instrument_id) = current.down_instrument_id
        && next.down_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.unsubscribe_book_deltas(instrument_id, None, None);
        #[cfg(not(test))]
        strategy.unsubscribe_trades(instrument_id, None, None);
        strategy.active.trade_flow.remove(&instrument_id);
        strategy.record_book_subscription_event(BookSubscriptionEvent::unsubscribe(instrument_id));
    }
    if let Some(instrument_id) = current.tracked_position_instrument_id
        && next.tracked_position_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.unsubscribe_book_deltas(instrument_id, None, None);
        #[cfg(not(test))]
        strategy.unsubscribe_trades(instrument_id, None, None);
        strategy.active.trade_flow.remove(&instrument_id);
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
        #[cfg(not(test))]
        strategy.subscribe_trades(instrument_id, None, None);
        let trade_flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&strategy.config));
        strategy.active.trade_flow.insert(instrument_id, trade_flow);
        strategy.record_book_subscription_event(BookSubscriptionEvent::subscribe(instrument_id));
    }
    if let Some(instrument_id) = next.down_instrument_id
        && current.down_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.subscribe_book_deltas(instrument_id, BookType::L2_MBP, None, None, false, None);
        #[cfg(not(test))]
        strategy.subscribe_trades(instrument_id, None, None);
        let trade_flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&strategy.config));
        strategy.active.trade_flow.insert(instrument_id, trade_flow);
        strategy.record_book_subscription_event(BookSubscriptionEvent::subscribe(instrument_id));
    }
    if let Some(instrument_id) = next.tracked_position_instrument_id
        && current.tracked_position_instrument_id != Some(instrument_id)
    {
        #[cfg(not(test))]
        strategy.subscribe_book_deltas(instrument_id, BookType::L2_MBP, None, None, false, None);
        #[cfg(not(test))]
        strategy.subscribe_trades(instrument_id, None, None);
        let trade_flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&strategy.config));
        strategy.active.trade_flow.insert(instrument_id, trade_flow);
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

const ORDER_SIDE_BUY_VALUE: &str = stringify!(buy);
const ORDER_SIDE_SELL_VALUE: &str = stringify!(sell);
const POSITION_SIDE_LONG_VALUE: &str = stringify!(long);
const POSITION_SIDE_SHORT_VALUE: &str = stringify!(short);

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

fn parse_configured_oms_type(field: &str, value: &str) -> Result<NtOmsType> {
    value
        .parse::<NtOmsType>()
        .with_context(|| format!("{field} must be a NautilusTrader OmsType, got `{value}`"))
}

fn expire_time_from_config(value: Option<u64>) -> Option<UnixNanos> {
    value.map(UnixNanos::from)
}

fn trigger_price_from_config(
    prefix: &'static str,
    trigger_price: Option<f64>,
    price_precision: u8,
) -> Result<Option<Price>> {
    price_from_config(prefix, "trigger_price", trigger_price, price_precision)
}

fn activation_price_from_config(
    prefix: &'static str,
    activation_price: Option<f64>,
    price_precision: u8,
) -> Result<Option<Price>> {
    price_from_config(
        prefix,
        "activation_price",
        activation_price,
        price_precision,
    )
}

fn price_from_config(
    prefix: &'static str,
    field: &'static str,
    value: Option<f64>,
    price_precision: u8,
) -> Result<Option<Price>> {
    value
        .map(|value| {
            anyhow::ensure!(
                is_positive_finite(value),
                "{prefix}_{field} must be positive and finite"
            );
            Ok(Price::new(value, price_precision))
        })
        .transpose()
}

fn trailing_offset_from_config(
    prefix: &'static str,
    trailing_offset: Option<f64>,
) -> Result<Option<Decimal>> {
    trailing_offset
        .map(|value| {
            anyhow::ensure!(
                is_positive_finite(value),
                "{prefix}_trailing_offset must be positive and finite"
            );
            Decimal::from_f64(value)
                .ok_or_else(|| anyhow::anyhow!("{prefix}_trailing_offset must be decimal"))
        })
        .transpose()
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

const INITIAL_COUNTER_USIZE: usize = 0;
const INITIAL_COUNTER_U64: u64 = 0;
const COUNTER_INCREMENT: usize = 1;
const COUNTER_INCREMENT_U64: u64 = 1;
const NANOS_PER_MILLI_U64: u64 = 1_000_000;
const NANOS_PER_SECOND_U64: u64 = 1_000_000_000;
const CONFIG_FIELD_OMS_TYPE: &str = "oms_type";
const CONFIG_FIELD_ENTRY_ORDER_SIDE: &str = "entry_order_side";
const CONFIG_FIELD_ENTRY_ORDER_POSITION_SIDE: &str = "entry_order_position_side";
const CONFIG_FIELD_EXIT_ORDER_SIDE: &str = "exit_order_side";
const CONFIG_FIELD_EXIT_ORDER_POSITION_SIDE: &str = "exit_order_position_side";
const CONFIG_FIELD_FORCED_EXIT_ORDER_SIDE: &str = "forced_exit_order_side";
const CONFIG_FIELD_FORCED_EXIT_ORDER_POSITION_SIDE: &str = "forced_exit_order_position_side";
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
const EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING: &str = "entry_order_still_working";
const EXIT_BLOCK_REASON_EXIT_DECISION_UNAVAILABLE: &str = "exit_decision_unavailable";
const EXIT_BLOCK_REASON_EXIT_HOLD: &str = "exit_hold";
const EXIT_BLOCK_REASON_OPEN_POSITION_MISSING: &str = "open_position_missing";
const EXIT_BLOCK_REASON_EXIT_ORDER_CONFIG_INVALID: &str = "exit_order_config_invalid";
const EXIT_BLOCK_REASON_EXIT_QUOTE_QUANTITY_UNSUPPORTED: &str = "exit_quote_quantity_unsupported";
const EXIT_BLOCK_REASON_EXIT_PRICE_MISSING: &str = "exit_price_missing";
const EXIT_BLOCK_REASON_EXIT_QUANTITY_NOT_POSITIVE: &str = "exit_quantity_not_positive";

fn evidence_number(value: f64) -> String {
    value.to_string()
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
    BookCrossed,
    IntervalOpenMissing,
    WarmupIncomplete,
    FeesNotReady,
    RecoveryMode,
    MarketCoolingDown,
    SpotSpikeCooldown,
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

#[derive(Debug, Clone, Copy, PartialEq)]
struct ExitOrderExecutionConfig {
    side: OrderSide,
    position_side: PositionSide,
    order_template: ConfiguredNtOrderTemplate,
}

impl ExitOrderExecutionConfig {
    fn nt_order_template(
        &self,
        prefix: &'static str,
        price_precision: u8,
    ) -> Result<NtOrderTemplate> {
        self.order_template
            .nt_order_template(prefix, price_precision)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct ExitSubmissionDecision {
    evaluation: ExitEvaluation,
    instrument_id: Option<InstrumentId>,
    order_type: Option<OrderType>,
    order_side: Option<OrderSide>,
    position_side: Option<PositionSide>,
    time_in_force: Option<TimeInForce>,
    price: Option<f64>,
    quantity: Option<Quantity>,
    client_order_id: Option<ClientOrderId>,
    is_post_only: Option<bool>,
    is_reduce_only: Option<bool>,
    is_quote_quantity: Option<bool>,
    expire_time_unix_nanos: Option<u64>,
    trigger_price: Option<f64>,
    activation_price: Option<f64>,
    trigger_type: Option<TriggerType>,
    trigger_instrument_id: Option<InstrumentId>,
    trailing_offset: Option<f64>,
    trailing_offset_type: Option<TrailingOffsetType>,
    blocked_reason: Option<&'static str>,
    forced_flat_reasons: Vec<ForcedFlatReason>,
}

impl ExitSubmissionDecision {
    fn execution_config(&self) -> Option<ExitOrderExecutionConfig> {
        Some(ExitOrderExecutionConfig {
            side: self.order_side?,
            position_side: self.position_side?,
            order_template: ConfiguredNtOrderTemplate {
                order_type: self.order_type?,
                time_in_force: self.time_in_force?,
                expire_time_unix_nanos: self.expire_time_unix_nanos,
                trigger_price: self.trigger_price,
                activation_price: self.activation_price,
                trigger_type: self.trigger_type,
                trigger_instrument_id: self.trigger_instrument_id,
                trailing_offset: self.trailing_offset,
                trailing_offset_type: self.trailing_offset_type,
                is_post_only: self.is_post_only?,
                is_reduce_only: self.is_reduce_only?,
                is_quote_quantity: self.is_quote_quantity?,
            },
        })
    }
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

fn should_warn_on_exit_submission_block(reason: Option<&str>) -> bool {
    !matches!(reason, Some(reason) if reason == EXIT_BLOCK_REASON_NO_OPEN_POSITION
        || reason == EXIT_BLOCK_REASON_EXIT_ALREADY_PENDING
        || reason == EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING
        || reason == EXIT_BLOCK_REASON_EXIT_HOLD)
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
    // Defense-in-depth (A14): a MISSING reference timestamp is the maximally
    // stale condition — the strategy has never observed a reference quote — so
    // it must classify as stale, not fresh. `is_none_or` returns `true` for the
    // `None` case (no reference ever) AND for an observed-but-aged reference,
    // and `false` only for a reference observed within the freshness bound.
    let reference_stale = inputs.last_reference_ts_ms.is_none_or(|last_ts_ms| {
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

#[cfg(test)]
fn submit_admission_request_from_order(
    intent: &BoltV3OrderIntentEvidence,
    order: &nautilus_model::orders::OrderAny,
) -> Result<BoltV3SubmitAdmissionRequest> {
    let client_order_id = order.client_order_id().to_string();
    let quantity_source = order.quantity().to_string();
    let quantity = Decimal::from_str(quantity_source.trim()).with_context(|| {
        format!(
            "bolt-v3 submit admission quantity is not a decimal for client_order_id={}",
            client_order_id
        )
    })?;
    // Base-only test helper: it has no strategy cache/instrument context, so it
    // cannot size quote-quantity orders (that requires the full
    // `admission_base_notional_from_order` path with an instrument) and it cannot
    // value a market-style order — ENTRY OR EXIT (production values any price-less
    // base-quantity order at the instrument's structural price ceiling via the
    // strategy method). It is NOT a divergent copy of the notional
    // math — for the shapes it DOES accept (base-quantity firm-limit orders) it
    // reuses the shared base-quantity definition so the order is sized
    // identically here, in the production strategy, and in the canary proof
    // executor.
    anyhow::ensure!(
        !order.is_quote_quantity(),
        "test submit admission helper requires strategy cache context for quote-quantity orders"
    );
    anyhow::ensure!(
        order.price().is_some(),
        "test submit admission helper cannot value a market-style order (no firm limit price): production values it at the instrument price ceiling — use `strategy.submit_admission_request_from_order` with a cache-seeded instrument"
    );
    let price_source = compiled_order_price_source(intent.price.clone(), order);
    let price = Decimal::from_str(price_source.trim()).with_context(|| {
        format!(
            "bolt-v3 submit admission price is not a decimal for client_order_id={}",
            client_order_id
        )
    })?;
    let notional = base_quantity_admission_notional(price, quantity);

    Ok(BoltV3SubmitAdmissionRequest {
        strategy_id: intent.strategy_id.clone(),
        client_order_id,
        instrument_id: order.instrument_id().to_string(),
        notional,
        order_side: order.order_side(),
        order_quantity: quantity,
        intent_kind: match intent.intent_kind {
            BoltV3OrderIntentKind::Entry => BoltV3SubmitIntentKind::Entry,
            BoltV3OrderIntentKind::Exit => BoltV3SubmitIntentKind::RiskReducingExit,
        },
        lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(true),
        canary_proof_claim: None,
        risk_reducing_exit_proof: None,
        position_sizing: None,
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
    use nautilus_common::{
        cache::Cache,
        clock::TestClock,
        messages::execution::TradingCommand,
        msgbus::{self, MessagingSwitchboard, stubs::get_typed_into_message_saving_handler},
    };
    use nautilus_core::{Params, UnixNanos};
    use nautilus_model::{
        enums::AssetClass,
        identifiers::{Symbol, TraderId},
        instruments::BinaryOption,
        orders::{Order, OrderAny},
        position::Position,
        types::{Currency, Price, Quantity},
    };
    use nautilus_portfolio::portfolio::Portfolio;
    use rust_decimal::Decimal;

    use super::*;
    use crate::{
        bolt_v3_submit_admission::BoltV3OrderLifecycleIntent,
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
            strategy_id = "BINARYORACLEEDGETAKER-001"
            order_id_tag = "001"
            oms_type = "netting"
            client_id = "POLYMARKET"
            configured_target_id = "configured_updown_target"
            target_kind = "rotating_market"
            rotating_market_family = "updown"
            underlying_asset = "CONFIGURED_ASSET"
            cadence_seconds = 300
            cadence_slug_token = "configuredwindow"
            market_selection_rule = "active_or_next"
            retry_interval_seconds = 5
            blocked_after_seconds = 60
            reference_venue = "reference_data_client"
            reference_instrument_id = "REFERENCE.SOURCE"
            use_uuid_client_order_ids = true
            use_hyphens_in_client_order_ids = false
            external_order_claims = ["AUXILIARY.SOURCE"]
            manage_contingent_orders = true
            manage_gtd_expiry = true
            manage_stop = true
            market_exit_interval_ms = 250
            market_exit_max_attempts = 7
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
            trade_flow_window_secs = 30
            trade_flow_max_samples = 100
            spike_guard_return_threshold = 0.05
            spike_guard_cooldown_secs = 5
            price_to_beat_source = "chainlink_data_streams.report_at_boundary"
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

            [forced_exit_order]
            side = "sell"
            position_side = "long"
            order_type = "market"
            time_in_force = "ioc"
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
        entry_fees: Mutex<HashMap<String, Decimal>>,
        warm_calls: Mutex<Vec<String>>,
    }

    impl RecordingFeeProvider {
        fn cold() -> Arc<Self> {
            Arc::new(Self::default())
        }

        /// Build a provider that yields `fee_bps` for `instrument_id`. The
        /// submit-admission path resolves the fee-inclusive notional through
        /// `FeeProvider::max_entry_fee_bps`, which falls back to `fee_bps` in
        /// `#[cfg(test)]` when the NT cache holds no instrument, so seeding the
        /// fee here is sufficient to exercise the fee-inclusive cap check
        /// without registering a full cache.
        fn with_fee(instrument_id: &str, fee_bps: Decimal) -> Arc<Self> {
            let provider = Arc::new(Self::default());
            provider.set_fee(instrument_id, fee_bps);
            provider
        }

        fn set_fee(&self, instrument_id: &str, fee_bps: Decimal) {
            self.fees
                .lock()
                .expect("recording fee provider mutex poisoned")
                .insert(instrument_id.to_string(), fee_bps);
        }

        fn set_entry_fee_bps(&self, instrument_id: &str, fee_bps: Decimal) {
            self.entry_fees
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

        fn entry_fee_bps(
            &self,
            instrument: &InstrumentAny,
            _entry_price: Decimal,
        ) -> Option<Decimal> {
            self.entry_fees
                .lock()
                .expect("recording fee provider mutex poisoned")
                .get(instrument.id().to_string().as_str())
                .copied()
                .or_else(|| self.fee_bps(instrument.id()))
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
        fn record_strategy_input_snapshot(
            &self,
            _snapshot: &crate::bolt_v3_decision_evidence::BoltV3StrategyInputEvidenceSnapshot,
        ) -> Result<()> {
            Ok(())
        }

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

        fn record_position_sizer_rebuild_audit(
            &self,
            _audit: &crate::bolt_v3_decision_evidence::BoltV3PositionSizerRebuildAuditEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_metadata(
            &self,
            _metadata: &crate::bolt_v3_decision_evidence::BoltV3SubmitReservationMetadataEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_fill(
            &self,
            _fill: &crate::bolt_v3_decision_evidence::BoltV3SubmitReservationFillEvidence,
        ) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FailingDecisionEvidenceWriter;

    impl crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter
        for FailingDecisionEvidenceWriter
    {
        fn record_strategy_input_snapshot(
            &self,
            _snapshot: &crate::bolt_v3_decision_evidence::BoltV3StrategyInputEvidenceSnapshot,
        ) -> Result<()> {
            anyhow::bail!("strategy input snapshot write failed")
        }

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

        fn record_position_sizer_rebuild_audit(
            &self,
            _audit: &crate::bolt_v3_decision_evidence::BoltV3PositionSizerRebuildAuditEvidence,
        ) -> Result<()> {
            anyhow::bail!("position sizer rebuild audit write failed")
        }

        fn record_submit_reservation_metadata(
            &self,
            _metadata: &crate::bolt_v3_decision_evidence::BoltV3SubmitReservationMetadataEvidence,
        ) -> Result<()> {
            anyhow::bail!("submit reservation metadata write failed")
        }

        fn record_submit_reservation_fill(
            &self,
            _fill: &crate::bolt_v3_decision_evidence::BoltV3SubmitReservationFillEvidence,
        ) -> Result<()> {
            anyhow::bail!("submit reservation fill write failed")
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    enum RecordedDecisionEvidenceEvent {
        StrategyInput(Box<crate::bolt_v3_decision_evidence::BoltV3StrategyInputEvidenceSnapshot>),
        OrderIntent(Box<crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence>),
        AdmissionDecision(crate::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence),
    }

    #[derive(Debug, Default)]
    struct RecordingSequencedDecisionEvidenceWriter {
        events: Mutex<Vec<RecordedDecisionEvidenceEvent>>,
    }

    impl RecordingSequencedDecisionEvidenceWriter {
        fn events(&self) -> Vec<RecordedDecisionEvidenceEvent> {
            self.events
                .lock()
                .expect("recording evidence writer mutex poisoned")
                .clone()
        }
    }

    impl crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter
        for RecordingSequencedDecisionEvidenceWriter
    {
        fn record_strategy_input_snapshot(
            &self,
            snapshot: &crate::bolt_v3_decision_evidence::BoltV3StrategyInputEvidenceSnapshot,
        ) -> Result<()> {
            self.events
                .lock()
                .expect("recording evidence writer mutex poisoned")
                .push(RecordedDecisionEvidenceEvent::StrategyInput(Box::new(
                    snapshot.clone(),
                )));
            Ok(())
        }

        fn record_order_intent(
            &self,
            intent: &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence,
        ) -> Result<()> {
            self.events
                .lock()
                .expect("recording evidence writer mutex poisoned")
                .push(RecordedDecisionEvidenceEvent::OrderIntent(Box::new(
                    intent.clone(),
                )));
            Ok(())
        }

        fn record_admission_decision(
            &self,
            decision: &crate::bolt_v3_decision_evidence::BoltV3AdmissionDecisionEvidence,
        ) -> Result<()> {
            self.events
                .lock()
                .expect("recording evidence writer mutex poisoned")
                .push(RecordedDecisionEvidenceEvent::AdmissionDecision(
                    decision.clone(),
                ));
            Ok(())
        }

        fn record_position_sizer_rebuild_audit(
            &self,
            _audit: &crate::bolt_v3_decision_evidence::BoltV3PositionSizerRebuildAuditEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_metadata(
            &self,
            _metadata: &crate::bolt_v3_decision_evidence::BoltV3SubmitReservationMetadataEvidence,
        ) -> Result<()> {
            Ok(())
        }

        fn record_submit_reservation_fill(
            &self,
            _fill: &crate::bolt_v3_decision_evidence::BoltV3SubmitReservationFillEvidence,
        ) -> Result<()> {
            Ok(())
        }
    }

    /// Execution venue of the binary-option market fixtures these tests trade against (their
    /// outcome instruments are `...POLYMARKET`). Production resolves the execution venue from
    /// config — `root.clients[execution_client_id].venue` — and is venue-agnostic (a HIP-4 or any
    /// other execution client works with no code change); these unit tests build the
    /// `StrategyBuildContext` directly without a root TOML, so they pin the venue to their fixtures
    /// in ONE place here rather than scattering the literal. A different-venue test would supply
    /// that venue plus matching instrument fixtures.
    fn fixture_execution_venue() -> Venue {
        Venue::from("POLYMARKET")
    }

    fn test_readiness_gate_evidence()
    -> crate::bolt_v3_decision_evidence::BoltV3ReadinessGateEvidenceSnapshot {
        crate::bolt_v3_decision_evidence::BoltV3ReadinessGateEvidenceSnapshot {
            gate_session_hash: "gate-session-hash-one".to_string(),
            selected_market_key: "selected-market-key-one".to_string(),
            gate_evidence: BTreeMap::from([(
                "resolution_price".to_string(),
                crate::bolt_v3_decision_evidence::BoltV3GateEvidenceIdentity {
                    satisfaction_kind: "evidence".to_string(),
                    selected_market_key: "selected-market-key-one".to_string(),
                    provider_id: Some("provider-one".to_string()),
                    provider_kind: Some("chainlink_data_streams".to_string()),
                    value_kind: Some("price".to_string()),
                    normalized_value_sha256: Some("normalized-value-sha-one".to_string()),
                    provider_provenance_sha256: Some("provider-provenance-sha-one".to_string()),
                    artifact_sha256s: vec!["artifact-sha-one".to_string()],
                    resolution_identity: None,
                },
            )]),
        }
    }

    fn test_strategy() -> BinaryOracleEdgeTaker {
        test_strategy_with_fee_provider(RecordingFeeProvider::cold())
    }

    fn test_strategy_with_runtime_readiness_seed(
        seed: BoltV3RuntimeReadinessSeed,
    ) -> BinaryOracleEdgeTaker {
        let mut strategy = test_strategy();
        strategy.context = strategy.context.clone().with_runtime_readiness_seed(seed);
        strategy
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
                configured_target_id: "configured_updown_target".to_string(),
                target_kind: "rotating_market".to_string(),
                rotating_market_family: "updown".to_string(),
                underlying_asset: "CONFIGURED_ASSET".to_string(),
                cadence_seconds: 300,
                cadence_slug_token: "configuredwindow".to_string(),
                market_selection_rule: "active_or_next".to_string(),
                retry_interval_seconds: 5,
                blocked_after_seconds: 60,
                reference_venue: Some("reference_data_client".to_string()),
                reference_instrument_id: Some("REFERENCE.SOURCE".to_string()),
                use_uuid_client_order_ids: true,
                use_hyphens_in_client_order_ids: false,
                external_order_claims: vec!["AUXILIARY.SOURCE".to_string()],
                manage_contingent_orders: true,
                manage_gtd_expiry: true,
                manage_stop: true,
                market_exit_interval_ms: 250,
                market_exit_max_attempts: 7,
                log_events: false,
                log_commands: false,
                log_rejected_due_post_only_as_warning: false,
                entry_order: BinaryOracleEdgeTakerOrderConfig {
                    side: "buy".to_string(),
                    position_side: "long".to_string(),
                    order_type: OrderType::Limit,
                    time_in_force: TimeInForce::Fok,
                    expire_time_unix_nanos: None,
                    trigger_price: None,
                    activation_price: None,
                    trigger_type: None,
                    trigger_instrument_id: None,
                    trailing_offset: None,
                    trailing_offset_type: None,
                    is_post_only: false,
                    is_reduce_only: false,
                    is_quote_quantity: false,
                },
                exit_order: BinaryOracleEdgeTakerOrderConfig {
                    side: "sell".to_string(),
                    position_side: "long".to_string(),
                    order_type: OrderType::Market,
                    time_in_force: TimeInForce::Ioc,
                    expire_time_unix_nanos: None,
                    trigger_price: None,
                    activation_price: None,
                    trigger_type: None,
                    trigger_instrument_id: None,
                    trailing_offset: None,
                    trailing_offset_type: None,
                    is_post_only: false,
                    is_reduce_only: false,
                    is_quote_quantity: false,
                },
                forced_exit_order: BinaryOracleEdgeTakerOrderConfig {
                    side: "sell".to_string(),
                    position_side: "long".to_string(),
                    order_type: OrderType::Market,
                    time_in_force: TimeInForce::Ioc,
                    expire_time_unix_nanos: None,
                    trigger_price: None,
                    activation_price: None,
                    trigger_type: None,
                    trigger_instrument_id: None,
                    trailing_offset: None,
                    trailing_offset_type: None,
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
                trade_flow_window_secs: 30,
                trade_flow_max_samples: 100,
                spike_guard_return_threshold: 0.05,
                spike_guard_cooldown_secs: 5,
                price_to_beat_source: "chainlink_data_streams.report_at_boundary".to_string(),
                pricing_kurtosis: 0.0,
                theta_decay_factor: 0.0,
                forced_flat_stale_reference_ms: 1500,
                forced_flat_thin_book_min_liquidity: 100.0,
                lead_agreement_min_corr: 0.8,
                lead_jitter_max_ms: 250,
            },
            StrategyBuildContext::new(
                fee_provider,
                decision_evidence,
                submit_admission,
                fixture_execution_venue(),
            )
            .with_readiness_evidence(test_readiness_gate_evidence()),
        )
    }

    #[test]
    fn strategy_core_uses_configured_nt_order_tag_and_oms_type() {
        let strategy = test_strategy();

        assert_eq!(strategy.core.config.order_id_tag.as_deref(), Some("001"));
        assert_eq!(strategy.core.config.oms_type, Some(NtOmsType::Netting));
    }

    #[test]
    fn submit_lifecycle_policy_is_derived_from_strategy_config() {
        let mut strategy = test_strategy();
        strategy.config.forced_exit_order.is_reduce_only = false;
        strategy.config.manage_contingent_orders = false;
        strategy.config.manage_gtd_expiry = false;
        strategy.config.manage_stop = false;
        let disabled = strategy.submit_lifecycle_policy();

        assert_eq!(disabled, BoltV3SubmitLifecyclePolicy::new(false));
        assert_eq!(
            disabled.submit_intent_for(BoltV3OrderLifecycleIntent::RiskReducingExit),
            Ok(Some(BoltV3SubmitIntentKind::RiskReducingExit))
        );
        assert_eq!(
            disabled.submit_intent_for(BoltV3OrderLifecycleIntent::ReplaceSubmit),
            Ok(None)
        );

        strategy.config.forced_exit_order.is_reduce_only = true;
        strategy.config.manage_contingent_orders = true;
        let enabled = strategy.submit_lifecycle_policy();
        assert_eq!(enabled, BoltV3SubmitLifecyclePolicy::new(true));
        assert_eq!(
            enabled.submit_intent_for(BoltV3OrderLifecycleIntent::ReplaceSubmit),
            Ok(Some(BoltV3SubmitIntentKind::ReplaceSubmit))
        );
    }

    #[test]
    fn strategy_core_accepts_nt_hedging_oms_type() {
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("raw config should be a TOML table")
            .insert("oms_type".to_string(), Value::String("hedging".to_string()));
        let config =
            BinaryOracleEdgeTakerBuilder::parse_config(&raw).expect("Hedging OMS should parse");
        let context = StrategyBuildContext::new(
            RecordingFeeProvider::cold(),
            Arc::new(RecordingDecisionEvidenceWriter),
            Arc::new(
                crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                    RecordingDecisionEvidenceWriter,
                )),
            ),
            fixture_execution_venue(),
        )
        .with_readiness_evidence(test_readiness_gate_evidence());

        let strategy = BinaryOracleEdgeTaker::new(config, context);

        assert_eq!(strategy.core.config.oms_type, Some(NtOmsType::Hedging));
    }

    #[test]
    fn parse_config_rejects_non_positive_spike_guard_return_threshold() {
        // A non-positive spike_guard_return_threshold makes the spike guard's
        // `relative_move >= threshold` test (relative_move is an abs(), always
        // >= 0) always true, arming the cooldown on every reference quote and
        // silently blocking all entry. 0.0 and negatives are valid TOML floats so
        // the type check cannot catch them; they must be rejected fail-loud at
        // config load.
        for bad in [0.0_f64, -0.01_f64] {
            let mut raw = valid_raw_config();
            raw.as_table_mut()
                .expect("raw config should be a TOML table")
                .insert(
                    "spike_guard_return_threshold".to_string(),
                    Value::Float(bad),
                );
            let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
                .expect_err("non-positive spike_guard_return_threshold must be rejected");
            assert!(
                err.to_string()
                    .contains("spike_guard_return_threshold must be positive and finite"),
                "expected positivity rejection for {bad}, got: {err}"
            );
        }
    }

    #[test]
    fn parse_config_rejects_zero_trade_flow_max_samples() {
        // A zero sample cap makes the count-cap evict every observation, leaving
        // the buffer permanently empty. 0 is a valid TOML integer so the type
        // check cannot catch it; it must be rejected fail-loud at config load.
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("raw config should be a TOML table")
            .insert("trade_flow_max_samples".to_string(), Value::Integer(0));
        let err = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
            .expect_err("zero trade_flow_max_samples must be rejected");
        assert!(
            err.to_string()
                .contains("trade_flow_max_samples must be positive"),
            "expected positivity rejection, got: {err}"
        );
    }

    #[test]
    fn runtime_config_parse_normalizes_order_fields_to_nt_enums() {
        let config = BinaryOracleEdgeTakerBuilder::parse_config(&valid_raw_config())
            .expect("valid raw config should parse into runtime config");

        assert_eq!(config.entry_order.order_type, OrderType::Limit);
        assert_eq!(config.entry_order.time_in_force, TimeInForce::Fok);
        assert_eq!(config.exit_order.order_type, OrderType::Market);
        assert_eq!(config.exit_order.time_in_force, TimeInForce::Ioc);
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
                fixture_execution_venue(),
            )
            .with_readiness_evidence(test_readiness_gate_evidence()),
        );

        assert!(strategy.core.config.use_uuid_client_order_ids);
        assert!(!strategy.core.config.use_hyphens_in_client_order_ids);
        assert_eq!(
            strategy.core.config.external_order_claims,
            Some(vec![InstrumentId::from("AUXILIARY.SOURCE")])
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

    fn trade_tick(instrument_id: &str, price: f64, ts_ms: u64) -> nautilus_model::data::TradeTick {
        trade_tick_with_aggressor(
            instrument_id,
            price,
            1.0,
            nautilus_model::enums::AggressorSide::Buyer,
            ts_ms,
        )
    }

    fn trade_tick_with_aggressor(
        instrument_id: &str,
        price: f64,
        size: f64,
        aggressor: nautilus_model::enums::AggressorSide,
        ts_ms: u64,
    ) -> nautilus_model::data::TradeTick {
        nautilus_model::data::TradeTick::new_checked(
            InstrumentId::from(instrument_id),
            Price::new(price, 2),
            Quantity::new(size, 0),
            aggressor,
            nautilus_model::identifiers::TradeId::from("TRADE-TICK-001"),
            nautilus_core::UnixNanos::from(ts_ms.saturating_mul(NANOS_PER_MILLI_U64)),
            nautilus_core::UnixNanos::from(ts_ms.saturating_mul(NANOS_PER_MILLI_U64)),
        )
        .expect("test trade tick should be valid")
    }

    #[test]
    fn reference_quote_tick_updates_pricing_from_configured_reference_data() {
        let mut strategy = test_strategy();

        strategy
            .on_quote(&quote_tick("REFERENCE.SOURCE", 100.0, 102.0, 1_200))
            .expect("reference quote should process");

        assert_eq!(strategy.pricing.last_reference_fair_value, Some(101.0));
        assert_eq!(
            strategy.pricing.fast_spot,
            Some(fast_spot("reference_data_client", 101.0, 1_200))
        );
    }

    #[test]
    fn non_reference_quote_tick_does_not_update_pricing() {
        let mut strategy = test_strategy();

        strategy
            .on_quote(&quote_tick("OTHER.SOURCE", 100.0, 102.0, 1_200))
            .expect("non-reference quote should be ignored");

        assert_eq!(strategy.pricing.last_reference_fair_value, None);
        assert_eq!(strategy.pricing.fast_spot, None);
    }

    #[test]
    fn source_owned_reference_identity_does_not_panic_nt_quote_filter() {
        let mut strategy = test_strategy();
        strategy.config.reference_venue = Some("resolution_oracle_primary".to_string());
        strategy.config.reference_instrument_id = Some("configured-reference-price".to_string());

        strategy
            .on_quote(&quote_tick("REFERENCE.SOURCE", 100.0, 102.0, 1_200))
            .expect("source-owned reference identity should not be parsed as an NT instrument");

        assert_eq!(strategy.pricing.last_reference_fair_value, None);
        assert_eq!(strategy.pricing.fast_spot, None);
    }

    #[test]
    fn source_owned_readiness_seed_warms_matching_runtime_market() {
        let market = candidate_market("market-1", 1_000);
        let seed = runtime_readiness_seed_for_market(&market, 3_100.0, 3_101.0, 1_200, 1.5);
        let mut strategy = test_strategy_with_runtime_readiness_seed(seed);

        strategy
            .apply_selection_snapshot(selection_snapshot(1_200, SelectionState::Active { market }));

        assert_eq!(strategy.active.price_to_beat, Some(3_100.0));
        assert_eq!(strategy.active.interval_open, Some(3_100.0));
        assert_eq!(strategy.active.last_reference_ts_ms, Some(1_200));
        assert_eq!(strategy.pricing.last_reference_fair_value, Some(3_101.0));
        assert_eq!(
            strategy.pricing.fast_spot,
            Some(fast_spot("reference_data_client", 3_101.0, 1_200))
        );
        assert_eq!(strategy.current_realized_vol_at(1_200), Some(1.5));
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

    fn submit_admission_armed_with_cap(
        max_notional_per_order: Decimal,
        decision_evidence: Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
    ) -> Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState> {
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(
                decision_evidence,
            ),
        );
        submit_admission
            .arm(live_canary_gate_report(1, max_notional_per_order))
            .expect("valid gate report should arm submit admission");
        submit_admission
    }

    #[test]
    fn decision_evidence_failure_rejects_before_nt_submit() {
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
        // Seed a (zero) fee for the order's instrument so admission clears the
        // fee-bound guard and the FailingDecisionEvidenceWriter is what rejects
        // the order before NT submit — the behavior this test pins.
        let mut strategy = test_strategy_with_fee_provider_and_decision_evidence(
            RecordingFeeProvider::with_fee(&instrument_id.to_string(), Decimal::ZERO),
            Arc::new(FailingDecisionEvidenceWriter),
        );
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
        let intent =
            crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            );

        let error = strategy
            .submit_order_with_decision_evidence(
                intent,
                order,
                SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            )
            .expect_err("evidence failure must reject before NT submit");

        assert!(
            error.to_string().contains("intent write failed"),
            "{error:#}"
        );
    }

    #[test]
    fn ungated_submit_admission_allows_after_evidence_before_nt_submit() {
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        );
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
        // Zero fee leaves the notional unchanged; with no optional gate armed,
        // production admission now allows the submit to reach NT.
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::with_fee(&instrument_id.to_string(), Decimal::ZERO),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
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
        let intent =
            crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            );

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            strategy.submit_order_with_decision_evidence(
                intent,
                order,
                SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            )
        }));

        assert!(
            result.is_err(),
            "test strategy is intentionally not registered with NT; ungated admission should reach NT submit"
        );
        assert_eq!(submit_admission.admitted_order_count(), 1);
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
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
        // Zero fee keeps the notional (0.50) under the 1.0 cap so admission
        // succeeds; the test then proves the submit reaches NT (and panics
        // because the strategy is intentionally unregistered).
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::with_fee(&instrument_id.to_string(), Decimal::ZERO),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
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
        let intent =
            crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            );

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            strategy.submit_order_with_decision_evidence(
                intent,
                order,
                SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            )
        }));

        assert!(
            result.is_err(),
            "test strategy is intentionally not registered with NT; reaching NT submit should panic"
        );
        assert_eq!(submit_admission.admitted_order_count(), 1);
    }

    #[test]
    fn effective_stale_bound_uses_gate_freshness_as_single_source_when_armed() {
        // A5: the gate-approved reference-quote freshness bound is the single
        // authoritative source for the armed live path, plumbed into the
        // forced-flat stale check as the STRICTER of (gate bound, strategy
        // config bound) so arming can only tighten, never loosen.
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

        // Unarmed: no gate bound exists, so the strategy config bound applies.
        strategy.config.forced_flat_stale_reference_ms = 1_500;
        assert_eq!(strategy.effective_stale_reference_after_ms(), 1_500);

        // Arm the gate. `for_test` carries reference_quote_max_age_seconds = 10
        // (10_000 ms). With a LARGER strategy config bound the gate value wins
        // (it tightens the stale check to the gate-approved freshness).
        submit_admission
            .arm(live_canary_gate_report(1, Decimal::new(1, 0)))
            .expect("valid gate report should arm submit admission");
        strategy.config.forced_flat_stale_reference_ms = 20_000;
        assert_eq!(
            strategy.effective_stale_reference_after_ms(),
            10_000,
            "armed gate freshness bound (10s) must tighten a looser strategy config bound (20s)"
        );

        // With a SMALLER strategy config bound the config value wins — arming
        // never loosens the existing strategy guard.
        strategy.config.forced_flat_stale_reference_ms = 1_500;
        assert_eq!(
            strategy.effective_stale_reference_after_ms(),
            1_500,
            "arming must never loosen a stricter strategy config freshness bound"
        );
    }

    #[test]
    fn submit_context_routes_non_empty_nt_params_to_submit_order() {
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        );
        submit_admission
            .arm(live_canary_gate_report(1, Decimal::new(1, 0)))
            .expect("valid gate report should arm submit admission");
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
        // Zero fee leaves the 0.50 notional under the 1.0 cap; this test
        // exercises submit-param routing, not the fee-inclusive cap.
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::with_fee(&instrument_id.to_string(), Decimal::ZERO),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
        let _cache = register_test_strategy(&mut strategy);
        let (risk_handler, risk_messages) =
            get_typed_into_message_saving_handler::<TradingCommand>(None);
        msgbus::register_trading_command_endpoint(
            MessagingSwitchboard::risk_engine_queue_execute(),
            risk_handler,
        );

        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-001-1");
        let order_side = strategy
            .configured_entry_order_side()
            .expect("test config should carry an entry order side");
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                order_side,
                quantity,
                price,
                client_order_id,
            )
            .expect("entry order should build through NT OrderFactory");
        let mut params = Params::new();
        params.insert(
            strategy.config.strategy_id.to_string(),
            serde_json::Value::Bool(true),
        );
        let intent =
            crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            );

        strategy
            .submit_order_with_decision_evidence(
                intent,
                order,
                SubmitContext::from_parts(
                    Some(ClientId::from("POLYMARKET")),
                    None,
                    Some(params.clone()),
                ),
            )
            .expect("non-empty submit params should reach NT submit");

        let messages = risk_messages.get_messages();
        let Some(TradingCommand::SubmitOrder(command)) = messages.first() else {
            panic!("expected one NT SubmitOrder command, got {messages:#?}");
        };
        assert_eq!(messages.len(), 1);
        assert_eq!(command.params.as_ref(), Some(&params));
        assert_eq!(submit_admission.admitted_order_count(), 1);
    }

    #[test]
    fn submit_admission_uses_compiled_limit_order_notional_not_prebuild_intent() {
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        );
        submit_admission
            .arm(live_canary_gate_report(1, Decimal::new(1, 0)))
            .expect("valid gate report should arm submit admission");
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
        // Zero fee isolates the assertion to "compiled order price drives the
        // notional": the compiled 2.0 notional still exceeds the 1.0 cap, while
        // the understated intent price (0.50) must NOT be what is checked.
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::with_fee(&instrument_id.to_string(), Decimal::ZERO),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(2.0, 2);
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
        let mut understated_intent =
            crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            );
        understated_intent.price = "0.50".to_string();

        let error = strategy
            .submit_order_with_decision_evidence(
                understated_intent,
                order,
                SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            )
            .expect_err("compiled order notional above cap must reject before NT submit");

        assert!(
            error.to_string().contains("notional cap is exceeded"),
            "{error:#}"
        );
        assert_eq!(submit_admission.admitted_order_count(), 0);
    }

    #[test]
    fn submit_sizing_evidence_uses_configured_outcome_metadata() {
        let strategy = ready_to_trade_strategy();
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(0.50, 2);
        let down_instrument_id = strategy
            .active
            .outcome_fees
            .down_instrument_id
            .expect("fixture should configure down outcome instrument");
        let order = nautilus_model::orders::OrderAny::Limit(
            nautilus_model::orders::LimitOrder::new_checked(
                nautilus_model::identifiers::TraderId::from("TRADER-001"),
                StrategyId::from(strategy.config.strategy_id.as_str()),
                down_instrument_id,
                ClientOrderId::from("O-19700101-000000-001-DOWN-1"),
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
            .expect("configured down outcome order should build"),
        );

        let evidence = strategy
            .compiled_order_sizing_evidence(&order, Decimal::new(1, 0), Decimal::new(50, 2))
            .expect("configured outcome order should produce sizing evidence");

        assert_eq!(
            evidence.prediction_market_outcome,
            Some(PredictionMarketOutcomeSide::No)
        );
    }

    #[test]
    fn quote_quantity_submit_admission_matches_nt_effective_notional_for_limit_buy() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let cache = register_test_strategy(&mut strategy);
        add_active_instruments_to_cache(&strategy, &cache);
        strategy.config.entry_order.is_quote_quantity = true;
        let instrument_id = selected_entry_instrument(&strategy);
        set_active_books_best_prices(&mut strategy, 0.24, 0.25);
        cache
            .borrow_mut()
            .add_quote(quote_tick(&instrument_id.to_string(), 0.24, 0.25, 1_200))
            .expect("test cache should accept quote tick");
        let quantity = Quantity::new(25.0, 2);
        let price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-1");
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                client_order_id,
            )
            .expect("quote-quantity limit order should build through the strategy factory path");
        assert!(order.is_quote_quantity());

        let admission = strategy
            .submit_admission_request_from_order(
                &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                    price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect("quote-quantity admission should use NT effective notional");

        assert_eq!(
            admission.notional,
            Decimal::from_str("50.00").expect("expected decimal should parse")
        );
    }

    #[test]
    fn quote_quantity_sell_limit_submit_admission_floors_to_quote_quantity() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let cache = register_test_strategy(&mut strategy);
        add_active_instruments_to_cache(&strategy, &cache);
        strategy.config.entry_order.is_quote_quantity = true;
        let instrument_id = selected_entry_instrument(&strategy);
        set_active_books_best_prices(&mut strategy, 0.75, 0.76);
        cache
            .borrow_mut()
            .add_quote(quote_tick(&instrument_id.to_string(), 0.75, 0.76, 1_200))
            .expect("test cache should accept quote tick");
        let submitted_quote_order_quantity = Quantity::new(25.0, 2);
        let limit_price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S1");
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Sell,
                submitted_quote_order_quantity,
                limit_price,
                client_order_id,
            )
            .expect(
                "quote-quantity sell limit order should build through the strategy factory path",
            );
        assert!(order.is_quote_quantity());

        let admission = strategy
            .submit_admission_request_from_order(
                &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                    limit_price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect(
                "quote-quantity sell limit admission should derive from compiled order context",
            );
        let submitted_quote_quantity = Decimal::from_str(order.quantity().to_string().trim())
            .expect("order quantity should parse as quote quantity");

        assert_eq!(
            admission.notional, submitted_quote_quantity,
            "SELL Limit admission must not understate submitted quote quantity when bid exceeds limit price"
        );
    }

    #[test]
    fn quote_quantity_sell_limit_missing_quote_uses_submitted_quote_quantity() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut strategy);
        strategy.config.entry_order.is_quote_quantity = true;
        let instrument_id = selected_entry_instrument(&strategy);
        set_active_books_best_prices(&mut strategy, 0.75, 0.76);
        let submitted_quote_order_quantity = Quantity::new(25.0, 2);
        let limit_price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S2");
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Sell,
                submitted_quote_order_quantity,
                limit_price,
                client_order_id,
            )
            .expect(
                "quote-quantity sell limit order should build through the strategy factory path",
            );
        assert!(order.is_quote_quantity());

        let admission = strategy
            .submit_admission_request_from_order(
                &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                    limit_price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect("missing quote should fall back to submitted quote quantity");

        assert_eq!(
            admission.notional,
            Decimal::from_str("25.00").expect("expected decimal should parse")
        );
    }

    #[test]
    fn quote_quantity_sell_limit_missing_context_fails_closed() {
        let mut builder = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut builder);
        builder.config.entry_order.is_quote_quantity = true;
        let instrument_id = selected_entry_instrument(&builder);
        set_active_books_best_prices(&mut builder, 0.75, 0.76);
        let submitted_quote_order_quantity = Quantity::new(25.0, 2);
        let limit_price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S3");
        let order = builder
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Sell,
                submitted_quote_order_quantity,
                limit_price,
                client_order_id,
            )
            .expect(
                "quote-quantity sell limit order should build through the strategy factory path",
            );
        assert!(order.is_quote_quantity());

        let mut strategy_without_instrument =
            ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let cache = register_test_strategy(&mut strategy_without_instrument);
        cache
            .borrow_mut()
            .add_quote(quote_tick(&instrument_id.to_string(), 0.75, 0.76, 1_200))
            .expect("test cache should accept quote tick");

        let error = strategy_without_instrument
            .submit_admission_request_from_order(
                &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy_without_instrument.config.strategy_id.clone(),
                    crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                    limit_price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect_err("missing instrument context must fail closed");

        assert!(
            error.to_string().contains("instrument context"),
            "{error:#}"
        );
    }

    #[test]
    fn quote_quantity_sell_stop_limit_submit_admission_floors_to_quote_quantity() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let cache = register_test_strategy(&mut strategy);
        add_active_instruments_to_cache(&strategy, &cache);
        strategy.config.entry_order.order_type = OrderType::StopLimit;
        strategy.config.entry_order.trigger_price = Some(0.52);
        strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
        strategy.config.entry_order.is_quote_quantity = true;
        let instrument_id = selected_entry_instrument(&strategy);
        set_active_books_best_prices(&mut strategy, 0.75, 0.76);
        cache
            .borrow_mut()
            .add_quote(quote_tick(&instrument_id.to_string(), 0.75, 0.76, 1_200))
            .expect("test cache should accept quote tick");
        let submitted_quote_order_quantity = Quantity::new(25.0, 2);
        let limit_price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S4");
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Sell,
                submitted_quote_order_quantity,
                limit_price,
                client_order_id,
            )
            .expect("quote-quantity sell stop-limit order should build through the strategy factory path");
        assert!(order.is_quote_quantity());
        assert!(matches!(order, OrderAny::StopLimit(_)));

        let admission = strategy
            .submit_admission_request_from_order(
                &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                    limit_price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect("quote-quantity sell stop-limit admission should derive from compiled order context");
        let submitted_quote_quantity = Decimal::from_str(order.quantity().to_string().trim())
            .expect("order quantity should parse as quote quantity");

        assert_eq!(
            admission.notional, submitted_quote_quantity,
            "SELL StopLimit admission must not understate submitted quote quantity when bid exceeds limit price"
        );
    }

    #[test]
    fn quote_quantity_sell_stop_limit_missing_quote_uses_submitted_quote_quantity() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut strategy);
        strategy.config.entry_order.order_type = OrderType::StopLimit;
        strategy.config.entry_order.trigger_price = Some(0.52);
        strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
        strategy.config.entry_order.is_quote_quantity = true;
        let instrument_id = selected_entry_instrument(&strategy);
        set_active_books_best_prices(&mut strategy, 0.75, 0.76);
        let submitted_quote_order_quantity = Quantity::new(25.0, 2);
        let limit_price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S5");
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Sell,
                submitted_quote_order_quantity,
                limit_price,
                client_order_id,
            )
            .expect("quote-quantity sell stop-limit order should build through the strategy factory path");
        assert!(order.is_quote_quantity());
        assert!(matches!(order, OrderAny::StopLimit(_)));

        let admission = strategy
            .submit_admission_request_from_order(
                &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                    limit_price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect("missing quote should fall back to submitted quote quantity");

        assert_eq!(
            admission.notional,
            Decimal::from_str("25.00").expect("expected decimal should parse")
        );
    }

    #[test]
    fn quote_quantity_sell_stop_limit_missing_context_fails_closed() {
        let mut builder = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut builder);
        builder.config.entry_order.order_type = OrderType::StopLimit;
        builder.config.entry_order.trigger_price = Some(0.52);
        builder.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
        builder.config.entry_order.is_quote_quantity = true;
        let instrument_id = selected_entry_instrument(&builder);
        set_active_books_best_prices(&mut builder, 0.75, 0.76);
        let submitted_quote_order_quantity = Quantity::new(25.0, 2);
        let limit_price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-S6");
        let order = builder
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Sell,
                submitted_quote_order_quantity,
                limit_price,
                client_order_id,
            )
            .expect("quote-quantity sell stop-limit order should build through the strategy factory path");
        assert!(order.is_quote_quantity());
        assert!(matches!(order, OrderAny::StopLimit(_)));

        let mut strategy_without_instrument =
            ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let cache = register_test_strategy(&mut strategy_without_instrument);
        cache
            .borrow_mut()
            .add_quote(quote_tick(&instrument_id.to_string(), 0.75, 0.76, 1_200))
            .expect("test cache should accept quote tick");

        let error = strategy_without_instrument
            .submit_admission_request_from_order(
                &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy_without_instrument.config.strategy_id.clone(),
                    crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                    limit_price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect_err("missing instrument context must fail closed");

        assert!(
            error.to_string().contains("instrument context"),
            "{error:#}"
        );
    }

    #[test]
    fn quote_quantity_submit_admission_uses_limit_price_when_nt_cache_quote_missing() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut strategy);
        strategy.config.entry_order.is_quote_quantity = true;
        let instrument_id = selected_entry_instrument(&strategy);
        set_active_books_best_prices(&mut strategy, 0.24, 0.25);
        let quantity = Quantity::new(25.0, 2);
        let price = Price::new(0.50, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-2");
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                client_order_id,
            )
            .expect("quote-quantity limit order should build through the strategy factory path");
        assert!(order.is_quote_quantity());

        let admission = strategy
            .submit_admission_request_from_order(
                &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                    price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect("quote-quantity admission should use NT no-quote fallback");

        assert_eq!(
            admission.notional,
            Decimal::from_str("25.00").expect("expected decimal should parse")
        );
    }

    #[test]
    fn quote_quantity_market_submit_admission_uses_nt_cache_quote_ask() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let cache = register_test_strategy(&mut strategy);
        add_active_instruments_to_cache(&strategy, &cache);
        strategy.config.entry_order.order_type = OrderType::Market;
        strategy.config.entry_order.time_in_force = TimeInForce::Ioc;
        strategy.config.entry_order.is_quote_quantity = true;
        let instrument_id = selected_entry_instrument(&strategy);
        let quote = quote_tick(&instrument_id.to_string(), 0.32, 0.33, 1_200);
        let expected_price = quote.ask_price;
        cache
            .borrow_mut()
            .add_quote(quote)
            .expect("test cache should accept quote tick");
        let quantity = Quantity::new(25.019, 3);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-3");
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                Price::new(0.99, 2),
                client_order_id,
            )
            .expect("quote-quantity market order should build through the strategy factory path");
        assert!(matches!(order, OrderAny::Market(_)));
        assert!(order.is_quote_quantity());
        let instrument = strategy
            .current_instrument(instrument_id)
            .expect("registered instrument should be available");
        let expected_notional = instrument
            .calculate_notional_value(
                instrument.calculate_base_quantity(order.quantity(), expected_price),
                expected_price,
                Some(true),
            )
            .as_decimal();
        let raw_quote_quantity = Decimal::from_str(order.quantity().to_string().trim())
            .expect("order quantity should parse");
        assert_ne!(expected_notional, raw_quote_quantity);

        let admission = strategy
            .submit_admission_request_from_order(
                &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                    "0.99".to_string(),
                    &order,
                ),
                &order,
            )
            .expect("quote-quantity market admission should use NT cache quote price");

        assert_eq!(admission.notional, expected_notional);
    }

    #[test]
    fn base_quantity_market_entry_admission_values_at_instrument_price_ceiling() {
        // A base-quantity Market entry has no firm limit price, so the venue fill
        // can land anywhere up to the instrument's structural price ceiling. The
        // admission notional must therefore be valued at that ceiling — the only
        // price the venue cannot exceed — not at the reference price the order
        // happens to be priced at. A firm-limit entry (fill <= limit) needs no
        // such adjustment; this test pins the ceiling valuation for the
        // market-style shape that lacks a firm price.
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let cache = register_test_strategy(&mut strategy);
        add_active_instruments_to_cache(&strategy, &cache);
        strategy.config.entry_order.order_type = OrderType::Market;
        strategy.config.entry_order.time_in_force = TimeInForce::Ioc;
        strategy.config.entry_order.is_quote_quantity = false;
        let instrument_id = selected_entry_instrument(&strategy);
        let quantity = Quantity::new(100.0, 2);
        let price = Price::new(0.33, 2);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-MKT-1");
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                client_order_id,
            )
            .expect("base-quantity market order should build through the strategy factory path");
        assert!(matches!(order, OrderAny::Market(_)));
        assert!(!order.is_quote_quantity());

        let admission = strategy
            .submit_admission_request_from_order(
                &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                    price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect("base-quantity market admission should value at the instrument price ceiling");

        // The fixture instrument declares max_price = 0.999 (the production NT
        // Polymarket adapter's ceiling), so the cap is valued at the ceiling
        // (0.999 * 100 = 99.9), NOT at the 0.33 reference price (33.00).
        assert_eq!(
            admission.notional,
            Decimal::from_str("99.9").expect("expected decimal should parse"),
            "a market-style base-quantity entry must be valued at qty * the instrument price ceiling"
        );
        assert!(
            admission.notional > price.as_decimal() * Decimal::from(100u32),
            "the ceiling valuation must bound strictly above the reference-price estimate it replaces"
        );
    }

    #[test]
    fn quote_quantity_market_submit_admission_uses_nt_cache_trade_when_quote_missing() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let cache = register_test_strategy(&mut strategy);
        add_active_instruments_to_cache(&strategy, &cache);
        strategy.config.entry_order.order_type = OrderType::Market;
        strategy.config.entry_order.time_in_force = TimeInForce::Ioc;
        strategy.config.entry_order.is_quote_quantity = true;
        let instrument_id = selected_entry_instrument(&strategy);
        let trade = trade_tick(&instrument_id.to_string(), 0.33, 1_200);
        let expected_price = trade.price;
        cache
            .borrow_mut()
            .add_trade(trade)
            .expect("test cache should accept trade tick");
        let quantity = Quantity::new(25.019, 3);
        let client_order_id = ClientOrderId::from("O-19700101-000000-001-QQQ-4");
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                Price::new(0.99, 2),
                client_order_id,
            )
            .expect("quote-quantity market order should build through the strategy factory path");
        assert!(matches!(order, OrderAny::Market(_)));
        assert!(order.is_quote_quantity());
        let instrument = strategy
            .current_instrument(instrument_id)
            .expect("registered instrument should be available");
        let expected_notional = instrument
            .calculate_notional_value(
                instrument.calculate_base_quantity(order.quantity(), expected_price),
                expected_price,
                Some(true),
            )
            .as_decimal();
        let raw_quote_quantity = Decimal::from_str(order.quantity().to_string().trim())
            .expect("order quantity should parse");
        assert_ne!(expected_notional, raw_quote_quantity);

        let admission = strategy
            .submit_admission_request_from_order(
                &crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                    "0.99".to_string(),
                    &order,
                ),
                &order,
            )
            .expect("quote-quantity market admission should use NT cache trade fallback");

        assert_eq!(admission.notional, expected_notional);
    }

    #[test]
    fn over_notional_submit_admission_rejects_before_nt_submit() {
        // Coarse over-cap rejection: raw notional = price*qty = 2.0*1.0 = 2.0
        // already exceeds the 1.0 cap, so this rejects with OR without the
        // fee-inclusive scaling and is NOT a discriminating test of the
        // fee multiplier at `binary_oracle_edge_taker.rs:3972`. The fee
        // multiplier is pinned by the discriminating pair
        // `fee_scaling_pushes_submit_admission_over_cap_rejects_before_nt_submit`
        // / `zero_fee_submit_admission_admits_at_same_cap` below; this test
        // only proves a grossly over-cap raw notional is rejected before NT
        // submit.
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        );
        submit_admission
            .arm(live_canary_gate_report(1, Decimal::ONE))
            .expect("valid gate report should arm submit admission");
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::with_fee(&instrument_id.to_string(), Decimal::new(25, 0)),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(2.0, 2);
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
        let intent =
            crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            );

        let error = strategy
            .submit_order_with_decision_evidence(
                intent,
                order,
                SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            )
            .expect_err("fee-inclusive over-cap notional must reject before NT submit");

        assert!(
            error.to_string().contains("notional cap is exceeded"),
            "{error:#}"
        );
        assert_eq!(submit_admission.admitted_order_count(), 0);
    }

    #[test]
    fn fee_inclusive_submit_admission_passes_at_cap_boundary() {
        // Strict-inequality boundary: raw notional = price*qty = 1.0*1.0 = 1.0;
        // at 25 bps the fee-inclusive admission notional is
        // 1.0 * (1 + 25/10000) = 1.0025, set EXACTLY equal to the cap. The
        // admission gate rejects only when notional STRICTLY exceeds the cap
        // (`request.notional > report.max_notional_per_order()`), so this must
        // be ADMITTED. The strategy is intentionally unregistered, so reaching
        // NT submit after a successful admission panics — proving admission
        // passed.
        //
        // NOTE: this is NOT a fee-discrimination test — with the cap set to the
        // fee-inclusive value, the raw 1.0 notional admits with OR without the
        // fee multiplier (1.0 <= 1.0025 either way). It only pins the strict
        // `>` boundary. Fee discrimination at
        // `binary_oracle_edge_taker.rs:3972` is pinned by the
        // `fee_scaling_pushes_submit_admission_over_cap_rejects_before_nt_submit`
        // / `zero_fee_submit_admission_admits_at_same_cap` pair below.
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        );
        submit_admission
            .arm(live_canary_gate_report(1, Decimal::new(10025, 4)))
            .expect("valid gate report should arm submit admission");
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::with_fee(&instrument_id.to_string(), Decimal::new(25, 0)),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(1.0, 2);
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
        let intent =
            crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            );

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            strategy.submit_order_with_decision_evidence(
                intent,
                order,
                SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            )
        }));

        assert!(
            result.is_err(),
            "fee-inclusive notional exactly at the cap must be admitted and reach NT submit (which panics on the unregistered test strategy)"
        );
        assert_eq!(
            submit_admission.admitted_order_count(),
            1,
            "fee-inclusive notional exactly at the cap must be admitted"
        );
    }

    #[test]
    fn fee_scaling_pushes_submit_admission_over_cap_rejects_before_nt_submit() {
        // DISCRIMINATING test for the fee-inclusive scaling at
        // `binary_oracle_edge_taker.rs:3972`. The cap is set STRICTLY BETWEEN the
        // raw notional and the fee-inclusive notional so the rejection is caused
        // by the fee multiplier specifically, not by the raw notional:
        //   raw notional      = price*qty = 1.00 * 1.00 = 1.0000
        //   cap               = 1.001  (Decimal::new(1001, 3))
        //   fee-inclusive     = 1.00 * (1 + 25/10000) = 1.0025
        // Since 1.0000 <= 1.001 < 1.0025, the order is admitted with the raw
        // notional but rejected with the fee-inclusive notional. This test must
        // FAIL if line 3972 is deleted: without the fee scaling the cap sees the
        // raw 1.0000 (<= 1.001) and admits, so the expected reject never fires.
        // The zero-fee CONTROL `zero_fee_submit_admission_admits_at_same_cap`
        // below uses the SAME raw 1.00 notional and SAME 1.001 cap and ADMITS,
        // proving the rejection here is caused by the fee scaling.
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        );
        submit_admission
            .arm(live_canary_gate_report(1, Decimal::new(1001, 3)))
            .expect("valid gate report should arm submit admission");
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::with_fee(&instrument_id.to_string(), Decimal::new(25, 0)),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(1.0, 2);
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
        let intent =
            crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            );

        let error = strategy
            .submit_order_with_decision_evidence(
                intent,
                order,
                SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            )
            .expect_err(
                "fee-inclusive notional pushed strictly over the cap by fee scaling must reject before NT submit",
            );

        assert!(
            error.to_string().contains("notional cap is exceeded"),
            "{error:#}"
        );
        assert_eq!(submit_admission.admitted_order_count(), 0);
    }

    #[test]
    fn zero_fee_submit_admission_admits_at_same_cap() {
        // Zero-fee CONTROL for
        // `fee_scaling_pushes_submit_admission_over_cap_rejects_before_nt_submit`.
        // SAME raw notional (price*qty = 1.00 * 1.00 = 1.0000) and SAME cap
        // (1.001) as the reject test, but the fee is ZERO so no scaling is
        // applied:
        //   admission notional = 1.0000 * (1 + 0/10000) = 1.0000 <= 1.001
        // and the order is ADMITTED. The strategy is intentionally unregistered,
        // so reaching NT submit after a successful admission panics — proving
        // admission passed. Together with the reject test this pair proves the
        // rejection there is caused by the fee scaling specifically, not by the
        // raw notional (which admits at this cap when the fee is zero).
        let submit_admission = Arc::new(
            crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                RecordingDecisionEvidenceWriter,
            )),
        );
        submit_admission
            .arm(live_canary_gate_report(1, Decimal::new(1001, 3)))
            .expect("valid gate report should arm submit admission");
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::with_fee(&instrument_id.to_string(), Decimal::ZERO),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(1.0, 2);
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
        let intent =
            crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            );

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            strategy.submit_order_with_decision_evidence(
                intent,
                order,
                SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            )
        }));

        assert!(
            result.is_err(),
            "zero-fee notional below the cap must be admitted and reach NT submit (which panics on the unregistered test strategy)"
        );
        assert_eq!(
            submit_admission.admitted_order_count(),
            1,
            "zero-fee notional below the cap must be admitted at the same cap the fee-scaled order is rejected at"
        );
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
                    order_side: OrderSide::Buy,
                    order_quantity: Decimal::new(1, 0),
                    intent_kind: crate::bolt_v3_submit_admission::BoltV3SubmitIntentKind::Entry,
                    lifecycle_policy:
                        crate::bolt_v3_submit_admission::BoltV3SubmitLifecyclePolicy::new(true),
                    canary_proof_claim: None,
                    risk_reducing_exit_proof: None,
                    position_sizing: None,
                },
            )
            .expect("first admission should consume the only slot");
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
        // Zero fee keeps the 0.50 notional under the 1.0 cap so the rejection is
        // the count-exhausted check, not the notional cap.
        let mut strategy = test_strategy_with_fee_provider_decision_evidence_and_submit_admission(
            RecordingFeeProvider::with_fee(&instrument_id.to_string(), Decimal::ZERO),
            Arc::new(RecordingDecisionEvidenceWriter),
            submit_admission.clone(),
        );
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
        let intent =
            crate::bolt_v3_decision_evidence::BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                crate::bolt_v3_decision_evidence::BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            );

        let error = strategy
            .submit_order_with_decision_evidence(
                intent,
                order,
                SubmitContext::with_client_id(ClientId::from("POLYMARKET")),
            )
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
        strategy.active.price_to_beat = Some(3_100.0);
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
        strategy.active.price_to_beat = Some(3_100.0);
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

    fn ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
        decision_evidence: Arc<dyn crate::bolt_v3_decision_evidence::BoltV3DecisionEvidenceWriter>,
        submit_admission: Arc<crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState>,
    ) -> BinaryOracleEdgeTaker {
        let (mut strategy, fee_provider) =
            ready_to_trade_strategy_with_recording_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.context = StrategyBuildContext::new(
            fee_provider,
            decision_evidence,
            submit_admission,
            fixture_execution_venue(),
        )
        .with_readiness_evidence(test_readiness_gate_evidence());
        strategy.config.edge_threshold_basis_points = 1;
        strategy.active.price_to_beat = Some(3_100.0);
        strategy
    }

    fn selected_entry_side(strategy: &BinaryOracleEdgeTaker) -> OutcomeSide {
        let evaluation = strategy.entry_evaluation_at(1_200);
        evaluation
            .selected_side
            .expect("ready-to-trade fixture should select a configured outcome side")
    }

    fn selected_entry_instrument(strategy: &BinaryOracleEdgeTaker) -> InstrumentId {
        strategy
            .entry_evaluation_at(1_200)
            .selected_side
            .and_then(|side| strategy.instrument_id_for_side(side))
            .or_else(|| configured_outcome_instruments(strategy).into_iter().next())
            .expect("ready-to-trade fixture should expose a configured instrument")
    }

    fn configured_position_probe(
        strategy: &mut BinaryOracleEdgeTaker,
        instrument_id: InstrumentId,
    ) -> OpenPositionState {
        let original_exposure = strategy.exposure.clone();
        strategy.materialize_position_from_event(
            instrument_id,
            PositionId::from("P-SIDE-PROBE"),
            OrderSide::Buy,
            PositionSide::Long,
            Quantity::new(1.0, 2),
            0.450,
        );
        let position = managed_position_ref(strategy)
            .cloned()
            .expect("configured instrument should materialize through production position path");
        strategy.exposure = original_exposure;
        strategy.refresh_book_subscriptions_for_current_state();
        position
    }

    fn configured_side_for_instrument(
        strategy: &mut BinaryOracleEdgeTaker,
        instrument_id: InstrumentId,
    ) -> OutcomeSide {
        configured_position_probe(strategy, instrument_id)
            .outcome_side
            .expect("configured instrument should materialize with an outcome side")
    }

    fn configured_book_for_instrument(
        strategy: &mut BinaryOracleEdgeTaker,
        instrument_id: InstrumentId,
    ) -> OutcomeBookState {
        configured_position_probe(strategy, instrument_id).book
    }

    fn set_active_books_best_prices(strategy: &mut BinaryOracleEdgeTaker, bid: f64, ask: f64) {
        let mut updates = Vec::new();
        for instrument_id in configured_outcome_instruments(strategy) {
            let book = configured_book_for_instrument(strategy, instrument_id);
            let bid_size = book
                .bid_levels
                .last_key_value()
                .map(|(_, size)| *size)
                .expect("configured fixture book should expose bid liquidity");
            let ask_size = book
                .ask_levels
                .first_key_value()
                .map(|(_, size)| *size)
                .expect("configured fixture book should expose ask liquidity");
            updates.push((instrument_id, bid_size, ask_size));
        }

        for (instrument_id, bid_size, ask_size) in updates {
            assert!(
                strategy.active.books.update_from_deltas(&book_deltas(
                    instrument_id,
                    &[
                        (BookAction::Clear, OrderSide::Buy, bid, bid_size),
                        (BookAction::Add, OrderSide::Buy, bid, bid_size),
                        (BookAction::Add, OrderSide::Sell, ask, ask_size),
                    ],
                )),
                "configured fixture book should accept price update"
            );
        }
    }

    fn set_configured_books_depth(
        strategy: &mut BinaryOracleEdgeTaker,
        deltas: &[(BookAction, OrderSide, f64, f64)],
    ) {
        for instrument_id in configured_outcome_instruments(strategy) {
            assert!(
                strategy
                    .active
                    .books
                    .update_from_deltas(&book_deltas(instrument_id, deltas)),
                "configured fixture book should accept depth update"
            );
        }
    }

    fn pending_entry_state(
        strategy: &mut BinaryOracleEdgeTaker,
        client_order_id: ClientOrderId,
    ) -> PendingEntryState {
        let instrument_id = selected_entry_instrument(strategy);
        let probe = configured_position_probe(strategy, instrument_id);
        let outcome_side = probe
            .outcome_side
            .expect("configured instrument should materialize with an outcome side");
        let book = probe.book;
        PendingEntryState {
            client_order_id,
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            outcome_side: Some(outcome_side),
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: strategy.entry_fee_bps(outcome_side).or(Some(0.0)),
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book,
        }
    }

    fn configured_instrument_except(
        strategy: &BinaryOracleEdgeTaker,
        instrument_id: InstrumentId,
    ) -> InstrumentId {
        configured_outcome_instruments(strategy)
            .into_iter()
            .find(|configured_instrument_id| *configured_instrument_id != instrument_id)
            .expect("fixture should expose a second configured outcome instrument")
    }

    fn foreign_venue_instrument_id(
        strategy: &BinaryOracleEdgeTaker,
        instrument_id: InstrumentId,
    ) -> InstrumentId {
        let foreign_instrument_id =
            InstrumentId::new(instrument_id.symbol, Venue::from("HYPERLIQUID"));
        assert_ne!(
            foreign_instrument_id.venue,
            strategy.context.execution_venue(),
            "foreign fixture instrument must not be on the execution venue",
        );
        foreign_instrument_id
    }

    fn materialize_configured_position(
        strategy: &mut BinaryOracleEdgeTaker,
        instrument_id: InstrumentId,
        position_id: PositionId,
        quantity: Quantity,
        avg_px_open: f64,
    ) -> OpenPositionState {
        strategy.materialize_position_from_event(
            instrument_id,
            position_id,
            OrderSide::Buy,
            PositionSide::Long,
            quantity,
            avg_px_open,
        );
        let mut position = managed_position_ref(strategy)
            .cloned()
            .expect("configured position should materialize as managed exposure");
        position.historical_entry_fee_bps.get_or_insert(0.0);
        position
    }

    fn configured_outcome_instruments(strategy: &BinaryOracleEdgeTaker) -> Vec<InstrumentId> {
        let instrument_ids = strategy.active.outcome_fees.instrument_ids();
        assert!(
            !instrument_ids.is_empty(),
            "ready-to-trade fixture should expose configured outcome instruments"
        );
        instrument_ids
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
        strategy.exposure = ExposureState::Managed(ManagedPositionState {
            position,
            origin,
            pending_entry: None,
        });
    }

    fn set_managed_position_with_pending_entry(
        strategy: &mut BinaryOracleEdgeTaker,
        position: OpenPositionState,
        origin: ManagedPositionOrigin,
        pending_entry: PendingEntryState,
    ) {
        strategy.exposure = ExposureState::Managed(ManagedPositionState {
            position,
            origin,
            pending_entry: Some(pending_entry),
        });
    }

    fn materialize_managed_position_with_resting_pending_entry(
        strategy: &mut BinaryOracleEdgeTaker,
        instrument_id: InstrumentId,
        position_id: PositionId,
        quantity: Quantity,
    ) -> (InstrumentId, ClientOrderId) {
        let client_order_id =
            ClientOrderId::from(format!("ENTRY-WORKING-{instrument_id}").as_str());
        let book = configured_book_for_instrument(strategy, instrument_id);
        let pending = PendingEntryState {
            client_order_id,
            market_id: Some("MKT-1".to_string()),
            instrument_id,
            outcome_side: None,
            outcome_fees: strategy.active.outcome_fees.clone(),
            historical_entry_fee_bps: strategy
                .context
                .fee_provider()
                .fee_bps(instrument_id)
                .and_then(|value| value.to_f64()),
            interval_open: Some(3_100.0),
            selection_published_at_ms: Some(1_000),
            seconds_to_expiry_at_selection: Some(300),
            book: book.clone(),
        };
        let avg_px_open = book
            .best_ask
            .expect("ready-to-trade fixture should expose an ask");
        set_pending_entry(strategy, pending);
        strategy.on_position_opened(position_opened_event(
            instrument_id,
            position_id,
            quantity,
            avg_px_open,
        ));
        (instrument_id, client_order_id)
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
                terminal_received: false,
                residual_position_observed_after_fill: false,
            },
            position: Some(ManagedPositionState {
                position,
                origin,
                pending_entry: None,
            }),
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

    fn assert_foreign_venue_blind_recovery(strategy: &BinaryOracleEdgeTaker) {
        assert!(
            matches!(
                strategy.exposure,
                ExposureState::BlindRecovery(BlindRecoveryState {
                    reason: BlindRecoveryReason::ForeignVenuePosition { .. }
                })
            ),
            "foreign-venue terminal event must be quarantined to blind recovery, got {:?}",
            strategy.exposure,
        );
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
            source_identity: SelectedMarketSourceIdentity {
                condition_id,
                market_slug: format!("slug-{market_id}"),
                question_id: format!("question-{market_id}"),
            },
            selection_outcome: MarketSelectionOutcome::Current,
            price_to_beat: None,
            start_ts_ms: interval_start_ms,
            expiration_ts_ms: interval_start_ms.saturating_add(300 * MILLIS_PER_SECOND_U64),
            seconds_to_end: 300,
        }
    }

    fn runtime_readiness_seed_for_market(
        market: &CandidateMarket,
        price_to_beat_value: f64,
        reference_price: f64,
        reference_quote_ts_event: u64,
        realized_volatility: f64,
    ) -> BoltV3RuntimeReadinessSeed {
        BoltV3RuntimeReadinessSeed {
            strategy_instance_id: "configured_updown_main".to_string(),
            gate_session_hash: "gate-session-hash-one".to_string(),
            selected_market_key: "selected-market-key-one".to_string(),
            polymarket_condition_id: market.source_identity.condition_id.clone(),
            polymarket_market_slug: market.source_identity.market_slug.clone(),
            polymarket_question_id: market.source_identity.question_id.clone(),
            up_instrument_id: market.up.instrument_id.clone(),
            down_instrument_id: market.down.instrument_id.clone(),
            market_start_timestamp_ms: market.start_ts_ms,
            market_end_timestamp_ms: market.expiration_ts_ms,
            price_to_beat_value,
            reference_venue: "reference_data_client".to_string(),
            reference_price,
            reference_quote_ts_event,
            realized_volatility,
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
        info.insert(
            "condition_id".to_string(),
            serde_json::Value::String(format!("condition-{market_id}")),
        );
        info.insert(
            "question_id".to_string(),
            serde_json::Value::String(format!("question-{market_id}")),
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
            // max_price: a binary option's structural payout ceiling. Mirrors the
            // upstream NT Polymarket adapter (MAX_PRICE = "0.999") so the fixture
            // declares the same hard price bound production instruments carry —
            // the only price a market-style order (BUY or SELL, entry or exit) can
            // ever fill or settle at, which the market-style admission valuation
            // uses as the universal worst case.
            Some(Price::from("0.999")),
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
    fn builder_rejects_unknown_rotating_market_family() {
        // P5-10: a market family not bound by the registry must be rejected at
        // parse, converging with the registry single source of truth.
        let mut raw = valid_raw_config();
        raw.as_table_mut()
            .expect("valid config must be a table")
            .insert(
                "rotating_market_family".to_string(),
                Value::String("not-a-real-family".to_string()),
            );
        let mut errors = Vec::new();

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        let error = find_error(
            &errors,
            "strategies[0].config.rotating_market_family",
            "unknown_market_family",
        );
        assert_eq!(error.message, "unknown market family `not-a-real-family`");
    }

    #[test]
    fn builder_accepts_registry_bound_rotating_market_family() {
        // The valid fixture's family is registry-bound, so no unknown-family
        // error is raised — the check must accept every family the registry
        // binds, not just reject unknown ones.
        let raw = valid_raw_config();
        let mut errors = Vec::new();

        BinaryOracleEdgeTakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);

        assert!(
            !errors
                .iter()
                .any(|error| error.code == "unknown_market_family"),
            "registry-bound family must not raise an unknown-market-family error: {errors:?}"
        );
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
                .any(|e| e.field == "strategies[0].config.trade_flow_window_secs")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.trade_flow_max_samples")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.spike_guard_return_threshold")
        );
        assert!(
            errors
                .iter()
                .any(|e| e.field == "strategies[0].config.spike_guard_cooldown_secs")
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
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
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
        let instrument_id = selected_entry_instrument(&strategy);
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
        let managed_position =
            managed_position_ref(&strategy).expect("position should be managed after open event");
        assert_eq!(managed_position.market_id.as_deref(), Some("MKT-1"));
        assert_eq!(managed_position.instrument_id, instrument_id);
        assert_eq!(managed_position.position_id, position_id);
        assert!(managed_position.outcome_side.is_some());
        assert_eq!(managed_position.outcome_fees, strategy.active.outcome_fees);
        assert_eq!(managed_position.historical_entry_fee_bps, None);
        assert_eq!(managed_position.entry_order_side, OrderSide::Buy);
        assert_eq!(managed_position.side, PositionSide::Long);
        assert_eq!(managed_position.quantity, Quantity::new(10.0, 2));
        assert_eq!(managed_position.avg_px_open, 0.450);
        assert_eq!(managed_position.interval_open, Some(3_100.0));
        assert_eq!(managed_position.selection_published_at_ms, Some(1_000));
        assert_eq!(managed_position.seconds_to_expiry_at_selection, Some(300));
        let managed_book = managed_position.book.clone();
        let expected_book = configured_book_for_instrument(&mut strategy, instrument_id);
        assert_eq!(managed_book, expected_book);

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
        let instrument_id = selected_entry_instrument(&strategy);
        let position_id = PositionId::from("P-EXIT-001");
        let exit_client_order_id = ClientOrderId::from("EXIT-001");
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        );
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
        let instrument_id = selected_entry_instrument(&strategy);
        let position_id = PositionId::from("P-EXIT-CHANGE");
        let exit_client_order_id = ClientOrderId::from("EXIT-CHANGE");
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        );
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
        let tracked_instrument = selected_entry_instrument(&strategy);
        let open_position = materialize_configured_position(
            &mut strategy,
            tracked_instrument,
            PositionId::from("P-TRACKED"),
            Quantity::new(10.0, 2),
            0.450,
        );
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
        let tracked_instrument = selected_entry_instrument(&strategy);
        let open_position = materialize_configured_position(
            &mut strategy,
            tracked_instrument,
            PositionId::from("P-TRACKED"),
            Quantity::new(10.0, 2),
            0.450,
        );
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
        let exit_client_order_id = ClientOrderId::from("EXIT-001");

        let mut canceled = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&canceled);
        let canceled_position = materialize_configured_position(
            &mut canceled,
            instrument_id,
            PositionId::from("P-CANCEL"),
            Quantity::new(1.0, 2),
            0.45,
        );
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
        let rejected_position = materialize_configured_position(
            &mut rejected,
            instrument_id,
            PositionId::from("P-REJECT"),
            Quantity::new(1.0, 2),
            0.45,
        );
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
        let expired_position = materialize_configured_position(
            &mut expired,
            instrument_id,
            PositionId::from("P-EXPIRE"),
            Quantity::new(1.0, 2),
            0.45,
        );
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
        let exit_client_order_id = ClientOrderId::from("EXIT-FILLED-CANCEL");

        let mut strategy = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&strategy);
        let position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-FILLED-CANCEL"),
            Quantity::new(1.0, 2),
            0.45,
        );
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
        let exit_client_order_id = ClientOrderId::from("EXIT-FILLED-REJECT");

        let mut strategy = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&strategy);
        let position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-FILLED-REJECT"),
            Quantity::new(1.0, 2),
            0.45,
        );
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
        let exit_client_order_id = ClientOrderId::from("EXIT-FILLED-EXPIRE");

        let mut strategy = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&strategy);
        let position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-FILLED-EXPIRE"),
            Quantity::new(1.0, 2),
            0.45,
        );
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
    fn partial_exit_fill_then_expire_restores_managed_residual_position() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&strategy);
        let position_id = PositionId::from("P-PARTIAL-EXIT-EXPIRE");
        let exit_client_order_id = ClientOrderId::from("EXIT-PARTIAL-EXPIRE");
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            position_id,
            Quantity::new(10.0, 2),
            0.45,
        );
        set_exit_pending(
            &mut strategy,
            open_position,
            exit_client_order_id,
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );

        let mut fill = order_filled_event_with_details(
            exit_client_order_id,
            instrument_id,
            Some(position_id),
            OrderSide::Sell,
        );
        fill.last_qty = Quantity::new(4.0, 2);
        strategy
            .on_order_filled(&fill)
            .expect("partial exit fill bookkeeping should succeed");
        strategy.materialize_position_from_event(
            instrument_id,
            position_id,
            OrderSide::Buy,
            PositionSide::Long,
            Quantity::new(6.0, 2),
            0.45,
        );

        strategy.on_order_expired(order_expired_event(exit_client_order_id, instrument_id));

        assert!(pending_exit_ref(&strategy).is_none());
        assert_eq!(
            strategy.exposure_occupancy(),
            Some(ExposureOccupancy::ManagedPosition)
        );
        assert_eq!(
            managed_position_ref(&strategy).map(|position| position.quantity),
            Some(Quantity::new(6.0, 2))
        );
    }

    #[test]
    fn exit_fill_quarantines_foreign_venue_client_order_id_collision() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&strategy);
        let position_id = PositionId::from("P-FOREIGN-EXIT-FILL");
        let exit_client_order_id = ClientOrderId::from("EXIT-FOREIGN-FILL");
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        );
        set_exit_pending(
            &mut strategy,
            open_position,
            exit_client_order_id,
            false,
            true,
            ManagedPositionOrigin::StrategyEntry,
        );
        let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

        strategy
            .on_order_filled(&order_filled_event(
                exit_client_order_id,
                foreign_instrument_id,
                position_id,
            ))
            .expect("foreign-venue exit fill should fail closed");

        assert_foreign_venue_blind_recovery(&strategy);
    }

    #[test]
    fn managed_entry_fill_quarantines_foreign_venue_client_order_id_collision() {
        let mut strategy = ready_to_trade_strategy();
        strategy.config.entry_order.time_in_force = TimeInForce::Ioc;
        let instrument_id = selected_entry_instrument(&strategy);
        let position_id = PositionId::from("P-FOREIGN-MANAGED-ENTRY-FILL");
        let entry_client_order_id = ClientOrderId::from("ENTRY-FOREIGN-MANAGED-FILL");
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        );
        let mut pending_entry = pending_entry_state(&mut strategy, entry_client_order_id);
        pending_entry.instrument_id = instrument_id;
        set_managed_position_with_pending_entry(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
            pending_entry,
        );
        let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

        strategy
            .on_order_filled(&order_filled_event_with_details(
                entry_client_order_id,
                foreign_instrument_id,
                Some(PositionId::from("P-FOREIGN-MANAGED-ENTRY-FILL")),
                OrderSide::Buy,
            ))
            .expect("foreign-venue managed entry fill should fail closed");

        assert_foreign_venue_blind_recovery(&strategy);
    }

    #[test]
    fn pending_entry_terminal_quarantines_foreign_venue_client_order_id_collision() {
        let mut strategy = ready_to_trade_strategy();
        let entry_client_order_id = ClientOrderId::from("ENTRY-FOREIGN-CANCEL");
        let pending_entry = pending_entry_state(&mut strategy, entry_client_order_id);
        let instrument_id = pending_entry.instrument_id;
        set_pending_entry(&mut strategy, pending_entry);
        let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

        strategy
            .on_order_canceled(&order_canceled_event(
                entry_client_order_id,
                foreign_instrument_id,
            ))
            .expect("foreign-venue entry cancel should fail closed");

        assert_foreign_venue_blind_recovery(&strategy);
    }

    #[test]
    fn managed_pending_entry_terminal_quarantines_foreign_venue_client_order_id_collision() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&strategy);
        let position_id = PositionId::from("P-FOREIGN-MANAGED-ENTRY-CANCEL");
        let entry_client_order_id = ClientOrderId::from("ENTRY-FOREIGN-MANAGED-CANCEL");
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        );
        let mut pending_entry = pending_entry_state(&mut strategy, entry_client_order_id);
        pending_entry.instrument_id = instrument_id;
        set_managed_position_with_pending_entry(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
            pending_entry,
        );
        let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

        strategy
            .on_order_canceled(&order_canceled_event(
                entry_client_order_id,
                foreign_instrument_id,
            ))
            .expect("foreign-venue managed entry cancel should fail closed");

        assert_foreign_venue_blind_recovery(&strategy);
    }

    #[test]
    fn exit_terminal_quarantines_foreign_venue_client_order_id_collision() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&strategy);
        let position_id = PositionId::from("P-FOREIGN-EXIT-CANCEL");
        let exit_client_order_id = ClientOrderId::from("EXIT-FOREIGN-CANCEL");
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        );
        set_exit_pending(
            &mut strategy,
            open_position,
            exit_client_order_id,
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );
        let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

        strategy
            .on_order_canceled(&order_canceled_event(
                exit_client_order_id,
                foreign_instrument_id,
            ))
            .expect("foreign-venue exit cancel should fail closed");

        assert_foreign_venue_blind_recovery(&strategy);
    }

    #[test]
    fn down_entry_submission_price_uses_configured_order_side_price() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        set_active_books_best_prices(&mut strategy, 0.40, 0.41);
        let selected_side = selected_entry_side(&strategy);
        assert_eq!(strategy.submission_entry_price(selected_side), Some(0.41));
        assert_eq!(strategy.executable_entry_cost(selected_side), Some(0.41));

        strategy.config.entry_order.side = "sell".to_string();
        strategy.config.entry_order.position_side = "short".to_string();
        strategy.config.exit_order.side = "buy".to_string();
        strategy.config.exit_order.position_side = "short".to_string();

        assert_eq!(strategy.submission_entry_price(selected_side), Some(0.40));
        assert_eq!(strategy.executable_entry_cost(selected_side), Some(0.40));
    }

    #[test]
    fn post_only_entry_submission_price_uses_passive_book_price() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.is_post_only = true;
        set_active_books_best_prices(&mut strategy, 0.40, 0.41);
        let selected_side = selected_entry_side(&strategy);

        assert_eq!(strategy.submission_entry_price(selected_side), Some(0.40));
        assert_eq!(strategy.executable_entry_cost(selected_side), Some(0.40));
    }

    #[test]
    fn stop_market_entry_submission_price_uses_trigger_price_for_notional_sizing() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.config.entry_order.order_type = OrderType::StopMarket;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.trigger_price = Some(0.52);
        set_active_books_best_prices(&mut strategy, 0.40, 0.41);
        let instrument_id = selected_entry_instrument(&strategy);
        let selected_side = configured_side_for_instrument(&mut strategy, instrument_id);

        assert_eq!(strategy.submission_entry_price(selected_side), Some(0.52));
        assert_eq!(strategy.executable_entry_cost(selected_side), Some(0.52));
    }

    #[test]
    fn quote_quantity_entry_submission_sizes_quantity_as_quote_notional() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut strategy);
        strategy.config.entry_order.is_quote_quantity = true;
        strategy.config.order_notional_target = 25.0;
        strategy.config.maximum_position_notional = 25.0;
        strategy.config.risk_lambda = 0.0001;

        let decision = strategy.entry_submission_decision_at(1_200);
        let sized_notional = decision
            .evaluation
            .sized_notional
            .expect("test setup should produce a positive sized notional");

        assert_eq!(decision.blocked_reason, None);
        assert!(
            decision
                .quantity_value
                .is_some_and(|quantity| (quantity - sized_notional).abs() < 1e-9),
            "quote-quantity entry should send quote notional as NT quantity, got {decision:#?}"
        );
    }

    #[test]
    fn market_if_touched_order_objects_preserve_nt_trigger_price_and_admission() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let cache = register_test_strategy(&mut strategy);
        add_active_instruments_to_cache(&strategy, &cache);
        strategy.config.entry_order.order_type = OrderType::MarketIfTouched;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.trigger_price = Some(0.52);
        strategy.config.entry_order.trigger_type = Some(TriggerType::MarkPrice);
        set_active_books_best_prices(&mut strategy, 0.40, 0.41);
        let instrument_id = selected_entry_instrument(&strategy);
        let selected_side = configured_side_for_instrument(&mut strategy, instrument_id);

        assert_eq!(strategy.submission_entry_price(selected_side), Some(0.52));
        assert_eq!(strategy.executable_entry_cost(selected_side), Some(0.52));

        let quantity = Quantity::new(2.0, 2);
        let fallback_price = Price::new(0.40, 2);
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                fallback_price,
                ClientOrderId::from("O-19700101-000000-001-006-1"),
            )
            .expect("MarketIfTouched order with explicit trigger price should build");

        let admission = strategy
            .submit_admission_request_from_order(
                &BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    BoltV3OrderIntentKind::Entry,
                    fallback_price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect("MarketIfTouched admission should derive from the instrument price ceiling");

        let OrderAny::MarketIfTouched(order) = order else {
            panic!("MarketIfTouched config should build an NT market-if-touched order");
        };
        assert_eq!(order.order_type(), OrderType::MarketIfTouched);
        assert_eq!(order.time_in_force(), TimeInForce::Gtc);
        assert_eq!(order.trigger_price(), Some(Price::new(0.52, 2)));
        assert_eq!(order.trigger_type(), Some(TriggerType::MarkPrice));
        assert_eq!(order.price(), None);
        assert!(!order.is_post_only());
        assert!(!order.is_reduce_only());
        assert!(!order.is_quote_quantity());
        assert_eq!(
            admission.notional,
            Decimal::from_str("1.998").expect("expected decimal should parse"),
            "a market-style MarketIfTouched entry must be valued at qty * the instrument price ceiling (2 * 0.999)"
        );
    }

    #[test]
    fn submit_admission_request_drops_canary_proof_order_intent_claim() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        let instrument_id = selected_entry_instrument(&strategy);
        let quantity = Quantity::new(2.0, 2);
        let price = Price::new(0.50, 2);
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-PROOF-1"),
            )
            .expect("configured entry order should build");
        let mut intent = BoltV3OrderIntentEvidence::from_compiled_order(
            strategy.config.strategy_id.clone(),
            BoltV3OrderIntentKind::Entry,
            price.to_string(),
            &order,
        );
        intent.canary_proof_claim = Some("proof_only".to_string());

        let admission = submit_admission_request_from_order(&intent, &order)
            .expect("entry intent should map into submit admission");

        assert_eq!(admission.canary_proof_claim, None);
    }

    #[test]
    fn market_if_touched_gtd_order_objects_preserve_nt_expire_time() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
        strategy.config.entry_order.order_type = OrderType::MarketIfTouched;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
        strategy.config.entry_order.trigger_price = Some(0.52);
        strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());

        let instrument_id = selected_entry_instrument(&strategy);
        let quantity = Quantity::new(2.0, 2);
        let fallback_price = Price::new(0.40, 2);
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                fallback_price,
                ClientOrderId::from("O-19700101-000000-001-008-1"),
            )
            .expect("MarketIfTouched GTD order with explicit expiry should build");

        let OrderAny::MarketIfTouched(order) = order else {
            panic!("MarketIfTouched GTD config should build an NT market-if-touched order");
        };
        assert_eq!(order.order_type(), OrderType::MarketIfTouched);
        assert_eq!(order.time_in_force(), TimeInForce::Gtd);
        assert_eq!(order.trigger_price(), Some(Price::new(0.52, 2)));
        assert_eq!(order.expire_time(), Some(expire_time));
    }

    #[test]
    fn post_only_exit_submission_price_uses_passive_book_price() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.config.exit_order.order_type = OrderType::Limit;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
        strategy.config.exit_order.is_post_only = true;
        set_active_books_best_prices(&mut strategy, 0.43, 0.51);
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_099.5, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
        let instrument_id = selected_entry_instrument(&strategy);
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-EXIT-001"),
            Quantity::new(10.0, 2),
            0.450,
        );
        let expected_passive_price = open_position.book.best_ask;
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );

        let decision = strategy.exit_submission_decision_at(1_200);

        assert!(decision.forced_flat_reasons.is_empty());
        assert_eq!(decision.order_side, Some(OrderSide::Sell));
        assert_eq!(decision.price, expected_passive_price);
    }

    #[test]
    fn exit_quote_quantity_config_is_blocked_before_base_position_quantity_is_used() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.config.exit_order.is_quote_quantity = true;
        strategy.config.exit_hysteresis_bps = 1_000_000;
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 3_099.5, 1_200));
        strategy.pricing.realized_vol.last_ready_vol = Some(2.5);
        strategy.pricing.realized_vol.last_ready_ts_ms = Some(1_200);
        let instrument_id = selected_entry_instrument(&strategy);
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-QUOTE-EXIT-001"),
            Quantity::new(10.0, 2),
            0.450,
        );
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );

        let decision = strategy.exit_submission_decision_at(1_200);

        assert_eq!(
            decision.blocked_reason,
            Some("exit_quote_quantity_unsupported")
        );
        assert_eq!(decision.quantity, None);
        assert_eq!(decision.is_quote_quantity, None);
    }

    #[test]
    fn exit_quote_quantity_order_build_is_rejected_before_nt_factory() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        strategy.config.exit_order.is_quote_quantity = true;
        let instrument_id = selected_entry_instrument(&strategy);
        let error = strategy
            .build_configured_exit_order(
                instrument_id,
                OrderSide::Sell,
                Quantity::new(10.0, 2),
                Price::new(0.50, 2),
                ClientOrderId::from("O-19700101-000000-001-QQE-1"),
            )
            .expect_err("exit quote-quantity should fail before NT factory construction");

        assert!(
            error.to_string().contains("exit_is_quote_quantity"),
            "{error:#}"
        );
    }

    #[test]
    fn reduce_only_entry_order_build_is_rejected_before_nt_factory() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        strategy.config.entry_order.is_reduce_only = true;
        let instrument_id = selected_entry_instrument(&strategy);
        let error = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                Quantity::new(10.0, 2),
                Price::new(0.50, 2),
                ClientOrderId::from("O-19700101-000000-001-ROE-1"),
            )
            .expect_err("reduce-only entry should fail before NT factory construction");

        assert!(
            error.to_string().contains("entry_is_reduce_only"),
            "{error:#}"
        );
    }

    #[test]
    fn forced_flat_exit_uses_forced_exit_order_when_normal_exit_is_post_only() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.active.phase = SelectionPhase::Freeze;
        strategy.config.exit_order.order_type = OrderType::Limit;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
        strategy.config.exit_order.is_post_only = true;
        strategy.config.forced_exit_order = strategy.config.exit_order.clone();
        strategy.config.forced_exit_order.order_type = OrderType::Market;
        strategy.config.forced_exit_order.time_in_force = TimeInForce::Ioc;
        strategy.config.forced_exit_order.is_post_only = false;
        strategy.config.forced_exit_order.is_reduce_only = true;
        set_active_books_best_prices(&mut strategy, 0.44, 0.45);
        let instrument_id = selected_entry_instrument(&strategy);
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-FORCED-EXIT-001"),
            Quantity::new(10.0, 2),
            0.450,
        );
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );

        let decision = strategy.exit_submission_decision_at(1_200);

        assert_eq!(decision.forced_flat_reasons, vec![ForcedFlatReason::Freeze]);
        assert_eq!(decision.order_type, Some(OrderType::Market));
        assert_eq!(decision.time_in_force, Some(TimeInForce::Ioc));
        assert_eq!(decision.is_post_only, Some(false));
        assert_eq!(decision.is_reduce_only, Some(true));
        assert_eq!(decision.price, Some(0.44));
    }

    #[test]
    fn forced_flat_exit_order_object_preserves_forced_exit_reduce_only_config() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut strategy);
        strategy.active.phase = SelectionPhase::Freeze;
        strategy.config.exit_order.order_type = OrderType::Limit;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
        strategy.config.exit_order.is_post_only = true;
        strategy.config.forced_exit_order = strategy.config.exit_order.clone();
        strategy.config.forced_exit_order.order_type = OrderType::Market;
        strategy.config.forced_exit_order.time_in_force = TimeInForce::Ioc;
        strategy.config.forced_exit_order.is_post_only = false;
        strategy.config.forced_exit_order.is_reduce_only = true;
        let instrument_id = selected_entry_instrument(&strategy);
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-FORCED-EXIT-ORDER-001"),
            Quantity::new(10.0, 2),
            0.450,
        );
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );
        let decision = strategy.exit_submission_decision_at(1_200);
        let price = Price::new(
            decision
                .price
                .expect("forced-flat decision should choose price"),
            strategy
                .current_instrument(instrument_id)
                .expect("test instrument should be registered")
                .price_precision(),
        );

        let order = strategy
            .build_exit_order_with_execution_config(
                decision
                    .execution_config()
                    .expect("forced-flat decision should carry order config"),
                instrument_id,
                decision.order_side.expect("forced-flat should choose side"),
                decision
                    .quantity
                    .expect("forced-flat should choose quantity"),
                price,
                ClientOrderId::from("O-19700101-000000-001-005-1"),
            )
            .expect("forced-flat market exit order should build");

        let OrderAny::Market(order) = order else {
            panic!("forced-flat exit should build an NT market order");
        };
        assert_eq!(order.order_type(), OrderType::Market);
        assert_eq!(order.time_in_force(), TimeInForce::Ioc);
        assert_eq!(order.price(), None);
        assert!(order.is_reduce_only());
        assert!(!order.is_quote_quantity());
    }

    #[test]
    fn forced_flat_exit_order_object_uses_configured_forced_exit_template() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        register_test_strategy_with_active_instruments(&mut strategy);
        strategy.active.phase = SelectionPhase::Freeze;
        strategy.config.exit_order.order_type = OrderType::Market;
        strategy.config.exit_order.time_in_force = TimeInForce::Ioc;
        strategy.config.exit_order.is_post_only = false;
        strategy.config.forced_exit_order = strategy.config.exit_order.clone();
        strategy.config.forced_exit_order.order_type = OrderType::Limit;
        strategy.config.forced_exit_order.time_in_force = TimeInForce::Gtc;
        strategy.config.forced_exit_order.is_post_only = true;
        strategy.config.forced_exit_order.is_reduce_only = true;
        let instrument_id = selected_entry_instrument(&strategy);
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-FORCED-EXIT-CONFIGURED-001"),
            Quantity::new(10.0, 2),
            0.450,
        );
        set_managed_position(
            &mut strategy,
            open_position,
            ManagedPositionOrigin::StrategyEntry,
        );
        let decision = strategy.exit_submission_decision_at(1_200);
        let price = Price::new(
            decision
                .price
                .expect("forced-flat decision should choose price"),
            strategy
                .current_instrument(instrument_id)
                .expect("test instrument should be registered")
                .price_precision(),
        );

        let order = strategy
            .build_exit_order_with_execution_config(
                decision
                    .execution_config()
                    .expect("forced-flat decision should carry order config"),
                instrument_id,
                decision.order_side.expect("forced-flat should choose side"),
                decision
                    .quantity
                    .expect("forced-flat should choose quantity"),
                price,
                ClientOrderId::from("O-19700101-000000-001-006-1"),
            )
            .expect("configured forced-flat exit order should build");

        let OrderAny::Limit(order) = order else {
            panic!("forced-flat exit should use configured NT order type");
        };
        assert_eq!(order.order_type(), OrderType::Limit);
        assert_eq!(order.time_in_force(), TimeInForce::Gtc);
        assert_eq!(order.price(), Some(price));
        assert!(order.is_post_only());
        assert!(order.is_reduce_only());
        assert!(!order.is_quote_quantity());
    }

    fn assert_limit_gtc_post_only_order(
        order: OrderAny,
        expected_side: OrderSide,
        expected_price: Price,
    ) {
        let OrderAny::Limit(order) = order else {
            panic!("maker order should be built as an NT limit order");
        };
        assert_eq!(order.order_side(), expected_side);
        assert_eq!(order.order_type(), OrderType::Limit);
        assert_eq!(order.time_in_force(), TimeInForce::Gtc);
        assert_eq!(order.price(), Some(expected_price));
        assert!(order.is_post_only());
        assert!(!order.is_reduce_only());
        assert!(!order.is_quote_quantity());
        assert_eq!(order.expire_time(), None);
    }

    #[test]
    fn post_only_maker_order_objects_preserve_nt_limit_gtc_fields() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.is_post_only = true;
        strategy.config.exit_order.order_type = OrderType::Limit;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
        strategy.config.exit_order.is_post_only = true;

        let instrument_id = selected_entry_instrument(&strategy);
        let quantity = Quantity::new(1.0, 2);
        let entry_price = Price::new(0.40, 2);
        let entry_order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                entry_price,
                ClientOrderId::from("O-19700101-000000-001-001-1"),
            )
            .expect("maker entry order should build");
        assert_limit_gtc_post_only_order(entry_order, OrderSide::Buy, entry_price);

        let exit_price = Price::new(0.45, 2);
        let exit_order = strategy
            .build_configured_exit_order(
                instrument_id,
                OrderSide::Sell,
                quantity,
                exit_price,
                ClientOrderId::from("O-19700101-000000-001-002-1"),
            )
            .expect("maker exit order should build");
        assert_limit_gtc_post_only_order(exit_order, OrderSide::Sell, exit_price);
    }

    #[test]
    fn gtd_limit_order_objects_preserve_nt_expire_time() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
        strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
        strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());

        let instrument_id = selected_entry_instrument(&strategy);
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(0.40, 2);
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-003-1"),
            )
            .expect("GTD limit order with explicit expiry should build");

        let OrderAny::Limit(order) = order else {
            panic!("GTD limit config should build an NT limit order");
        };
        assert_eq!(order.time_in_force(), TimeInForce::Gtd);
        assert_eq!(order.expire_time(), Some(expire_time));
    }

    #[test]
    fn non_gtd_limit_order_objects_preserve_nt_expire_time() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());
        let instrument_id = selected_entry_instrument(&strategy);

        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                Quantity::new(1.0, 2),
                Price::new(0.40, 2),
                ClientOrderId::from("O-19700101-000000-001-020-1"),
            )
            .expect("non-GTD limit expiry should pass through to NT");

        let OrderAny::Limit(order) = order else {
            panic!("non-GTD limit config should build an NT limit order");
        };
        assert_eq!(order.time_in_force(), TimeInForce::Gtc);
        assert_eq!(order.expire_time(), Some(expire_time));
    }

    #[test]
    fn stop_market_order_objects_preserve_nt_trigger_price_and_admission() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let cache = register_test_strategy(&mut strategy);
        add_active_instruments_to_cache(&strategy, &cache);
        strategy.config.entry_order.order_type = OrderType::StopMarket;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.trigger_price = Some(0.52);
        strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);

        let instrument_id = selected_entry_instrument(&strategy);
        let quantity = Quantity::new(2.0, 2);
        let admission_price = Price::new(0.40, 2);
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                admission_price,
                ClientOrderId::from("O-19700101-000000-001-005-1"),
            )
            .expect("StopMarket order with explicit trigger price should build");

        let admission = strategy
            .submit_admission_request_from_order(
                &BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    BoltV3OrderIntentKind::Entry,
                    admission_price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect("StopMarket admission should derive from the instrument price ceiling");

        let OrderAny::StopMarket(order) = order else {
            panic!("StopMarket config should build an NT stop-market order");
        };
        assert_eq!(order.order_type(), OrderType::StopMarket);
        assert_eq!(order.time_in_force(), TimeInForce::Gtc);
        assert_eq!(order.trigger_price(), Some(Price::new(0.52, 2)));
        assert_eq!(order.trigger_type(), Some(TriggerType::LastPrice));
        assert_eq!(order.price(), None);
        assert!(!order.is_reduce_only());
        assert!(!order.is_quote_quantity());
        assert_eq!(
            admission.notional,
            Decimal::from_str("1.998").expect("expected decimal should parse"),
            "a market-style StopMarket entry must be valued at qty * the instrument price ceiling (2 * 0.999)"
        );
    }

    #[test]
    fn triggered_order_objects_preserve_nt_trigger_instrument_id() {
        let mut raw = valid_raw_config();
        let entry_order = raw
            .as_table_mut()
            .expect("valid config must be a table")
            .get_mut("entry_order")
            .expect("valid config should include entry_order")
            .as_table_mut()
            .expect("entry_order should be a table");
        entry_order.insert(
            "order_type".to_string(),
            Value::String("stop_market".to_string()),
        );
        entry_order.insert(
            "time_in_force".to_string(),
            Value::String("gtc".to_string()),
        );
        entry_order.insert("trigger_price".to_string(), Value::Float(0.52));
        entry_order.insert(
            "trigger_instrument_id".to_string(),
            Value::String("TRIGGER.SOURCE".to_string()),
        );
        let config = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
            .expect("trigger_instrument_id should parse through runtime config");
        let context = StrategyBuildContext::new(
            RecordingFeeProvider::cold(),
            Arc::new(RecordingDecisionEvidenceWriter),
            Arc::new(
                crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                    RecordingDecisionEvidenceWriter,
                )),
            ),
            fixture_execution_venue(),
        )
        .with_readiness_evidence(test_readiness_gate_evidence());
        let mut strategy = BinaryOracleEdgeTaker::new(config, context);
        let _cache = register_test_strategy(&mut strategy);
        let trigger_instrument_id = InstrumentId::from("TRIGGER.SOURCE");
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());

        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                Quantity::new(2.0, 2),
                Price::new(0.40, 2),
                ClientOrderId::from("O-19700101-000000-001-021-1"),
            )
            .expect("triggered order should build with NT trigger_instrument_id");

        let OrderAny::StopMarket(order) = order else {
            panic!("StopMarket config should build an NT stop-market order");
        };
        assert_eq!(order.trigger_instrument_id(), Some(trigger_instrument_id));
    }

    #[test]
    fn non_triggered_order_rejects_trigger_instrument_id_before_factory() {
        let mut raw = valid_raw_config();
        let entry_order = raw
            .as_table_mut()
            .expect("valid config must be a table")
            .get_mut("entry_order")
            .expect("valid config should include entry_order")
            .as_table_mut()
            .expect("entry_order should be a table");
        entry_order.insert(
            "trigger_instrument_id".to_string(),
            Value::String("TRIGGER.SOURCE".to_string()),
        );
        let config = BinaryOracleEdgeTakerBuilder::parse_config(&raw)
            .expect("trigger_instrument_id should parse through runtime config");
        let context = StrategyBuildContext::new(
            RecordingFeeProvider::cold(),
            Arc::new(RecordingDecisionEvidenceWriter),
            Arc::new(
                crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                    RecordingDecisionEvidenceWriter,
                )),
            ),
            fixture_execution_venue(),
        )
        .with_readiness_evidence(test_readiness_gate_evidence());
        let mut strategy = BinaryOracleEdgeTaker::new(config, context);
        let _cache = register_test_strategy(&mut strategy);
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());

        let error = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                Quantity::new(2.0, 2),
                Price::new(0.40, 2),
                ClientOrderId::from("O-19700101-000000-001-022-1"),
            )
            .expect_err("non-triggered order must not silently carry trigger_instrument_id");

        assert!(
            error.to_string().contains("trigger_instrument_id"),
            "{error}"
        );
    }

    #[test]
    fn stop_limit_order_objects_preserve_nt_price_trigger_and_admission() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
        strategy.config.entry_order.order_type = OrderType::StopLimit;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
        strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());
        strategy.config.entry_order.trigger_price = Some(0.52);
        strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
        strategy.config.entry_order.is_post_only = true;
        strategy.config.exit_order.order_type = OrderType::StopLimit;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtd;
        strategy.config.exit_order.expire_time_unix_nanos = Some(expire_time.as_u64());
        strategy.config.exit_order.trigger_price = Some(0.48);
        strategy.config.exit_order.trigger_type = Some(TriggerType::MarkPrice);
        strategy.config.exit_order.is_post_only = true;

        let instrument_id = selected_entry_instrument(&strategy);
        let quantity = Quantity::new(2.0, 2);
        let price = Price::new(0.40, 2);
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-006-1"),
            )
            .expect("StopLimit order with explicit trigger price should build");

        let admission = submit_admission_request_from_order(
            &BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            ),
            &order,
        )
        .expect("StopLimit admission should derive from the compiled NT order");

        let OrderAny::StopLimit(order) = order else {
            panic!("StopLimit config should build an NT stop-limit order");
        };
        assert_eq!(order.order_type(), OrderType::StopLimit);
        assert_eq!(order.time_in_force(), TimeInForce::Gtd);
        assert_eq!(order.price(), Some(price));
        assert_eq!(order.trigger_price(), Some(Price::new(0.52, 2)));
        assert_eq!(order.trigger_type(), Some(TriggerType::LastPrice));
        assert_eq!(order.expire_time(), Some(expire_time));
        assert!(order.is_post_only());
        assert!(!order.is_reduce_only());
        assert!(!order.is_quote_quantity());
        assert_eq!(
            admission.notional,
            Decimal::from_str("0.800").expect("expected decimal should parse")
        );

        let exit_price = Price::new(0.45, 2);
        let exit_order = strategy
            .build_configured_exit_order(
                instrument_id,
                OrderSide::Sell,
                quantity,
                exit_price,
                ClientOrderId::from("O-19700101-000000-001-007-1"),
            )
            .expect("StopLimit exit order with explicit trigger price should build");

        let OrderAny::StopLimit(exit_order) = exit_order else {
            panic!("StopLimit exit config should build an NT stop-limit order");
        };
        assert_eq!(exit_order.order_side(), OrderSide::Sell);
        assert_eq!(exit_order.order_type(), OrderType::StopLimit);
        assert_eq!(exit_order.time_in_force(), TimeInForce::Gtd);
        assert_eq!(exit_order.price(), Some(exit_price));
        assert_eq!(exit_order.trigger_price(), Some(Price::new(0.48, 2)));
        assert_eq!(exit_order.trigger_type(), Some(TriggerType::MarkPrice));
        assert_eq!(exit_order.expire_time(), Some(expire_time));
        assert!(exit_order.is_post_only());
        assert!(!exit_order.is_reduce_only());
        assert!(!exit_order.is_quote_quantity());
    }

    #[test]
    fn limit_if_touched_order_objects_preserve_nt_price_trigger_and_admission() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
        strategy.config.entry_order.order_type = OrderType::LimitIfTouched;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
        strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());
        strategy.config.entry_order.trigger_price = Some(0.39);
        strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
        strategy.config.entry_order.is_post_only = true;
        strategy.config.exit_order.order_type = OrderType::LimitIfTouched;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtd;
        strategy.config.exit_order.expire_time_unix_nanos = Some(expire_time.as_u64());
        strategy.config.exit_order.trigger_price = Some(0.46);
        strategy.config.exit_order.trigger_type = Some(TriggerType::MarkPrice);
        strategy.config.exit_order.is_post_only = true;

        let instrument_id = selected_entry_instrument(&strategy);
        let quantity = Quantity::new(2.0, 2);
        let price = Price::new(0.40, 2);
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-009-1"),
            )
            .expect("LimitIfTouched entry order with explicit trigger price should build");

        let admission = submit_admission_request_from_order(
            &BoltV3OrderIntentEvidence::from_compiled_order(
                strategy.config.strategy_id.clone(),
                BoltV3OrderIntentKind::Entry,
                price.to_string(),
                &order,
            ),
            &order,
        )
        .expect("LimitIfTouched admission should derive from the compiled NT order");

        let OrderAny::LimitIfTouched(order) = order else {
            panic!("LimitIfTouched config should build an NT limit-if-touched order");
        };
        assert_eq!(order.order_side(), OrderSide::Buy);
        assert_eq!(order.order_type(), OrderType::LimitIfTouched);
        assert_eq!(order.time_in_force(), TimeInForce::Gtd);
        assert_eq!(order.price(), Some(price));
        assert_eq!(order.trigger_price(), Some(Price::new(0.39, 2)));
        assert_eq!(order.trigger_type(), Some(TriggerType::LastPrice));
        assert_eq!(order.expire_time(), Some(expire_time));
        assert!(order.is_post_only());
        assert!(!order.is_reduce_only());
        assert!(!order.is_quote_quantity());
        assert_eq!(
            admission.notional,
            Decimal::from_str("0.800").expect("expected decimal should parse")
        );

        let exit_price = Price::new(0.45, 2);
        let exit_order = strategy
            .build_configured_exit_order(
                instrument_id,
                OrderSide::Sell,
                quantity,
                exit_price,
                ClientOrderId::from("O-19700101-000000-001-010-1"),
            )
            .expect("LimitIfTouched exit order with explicit trigger price should build");

        let OrderAny::LimitIfTouched(exit_order) = exit_order else {
            panic!("LimitIfTouched exit config should build an NT limit-if-touched order");
        };
        assert_eq!(exit_order.order_side(), OrderSide::Sell);
        assert_eq!(exit_order.order_type(), OrderType::LimitIfTouched);
        assert_eq!(exit_order.time_in_force(), TimeInForce::Gtd);
        assert_eq!(exit_order.price(), Some(exit_price));
        assert_eq!(exit_order.trigger_price(), Some(Price::new(0.46, 2)));
        assert_eq!(exit_order.trigger_type(), Some(TriggerType::MarkPrice));
        assert_eq!(exit_order.expire_time(), Some(expire_time));
        assert!(exit_order.is_post_only());
        assert!(!exit_order.is_reduce_only());
        assert!(!exit_order.is_quote_quantity());
    }

    #[test]
    fn limit_if_touched_rejects_nt_side_price_invariants_before_factory() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        let instrument_id = selected_entry_instrument(&strategy);
        let quantity = Quantity::new(2.0, 2);

        strategy.config.entry_order.order_type = OrderType::LimitIfTouched;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.trigger_price = Some(0.41);
        let buy_error = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                Price::new(0.40, 2),
                ClientOrderId::from("O-19700101-000000-001-011-1"),
            )
            .expect_err(
                "BUY LimitIfTouched with trigger above limit should fail before NT factory",
            );
        assert!(
            buy_error.to_string().contains("trigger_price") && buy_error.to_string().contains("<="),
            "{buy_error}"
        );

        strategy.config.exit_order.order_type = OrderType::LimitIfTouched;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
        strategy.config.exit_order.trigger_price = Some(0.44);
        let sell_error = strategy
            .build_configured_exit_order(
                instrument_id,
                OrderSide::Sell,
                quantity,
                Price::new(0.45, 2),
                ClientOrderId::from("O-19700101-000000-001-012-1"),
            )
            .expect_err(
                "SELL LimitIfTouched with trigger below limit should fail before NT factory",
            );
        assert!(
            sell_error.to_string().contains("trigger_price")
                && sell_error.to_string().contains(">="),
            "{sell_error}"
        );
    }

    #[test]
    fn trailing_stop_market_order_objects_preserve_nt_trailing_fields_and_admission() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let cache = register_test_strategy(&mut strategy);
        add_active_instruments_to_cache(&strategy, &cache);
        let expire_time = nautilus_core::UnixNanos::from(4_102_444_800_000_000_000_u64);
        strategy.config.entry_order.order_type = OrderType::TrailingStopMarket;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
        strategy.config.entry_order.expire_time_unix_nanos = Some(expire_time.as_u64());
        strategy.config.entry_order.trigger_price = Some(0.52);
        strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
        strategy.config.entry_order.trailing_offset = Some(2.5);
        strategy.config.entry_order.trailing_offset_type = Some(TrailingOffsetType::BasisPoints);
        strategy.config.entry_order.is_post_only = false;
        strategy.config.exit_order.order_type = OrderType::TrailingStopMarket;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtd;
        strategy.config.exit_order.expire_time_unix_nanos = Some(expire_time.as_u64());
        strategy.config.exit_order.activation_price = Some(0.48);
        strategy.config.exit_order.trigger_type = Some(TriggerType::MarkPrice);
        strategy.config.exit_order.trailing_offset = Some(3.0);
        strategy.config.exit_order.trailing_offset_type = Some(TrailingOffsetType::Ticks);
        strategy.config.exit_order.is_post_only = false;

        let instrument_id = selected_entry_instrument(&strategy);
        let quantity = Quantity::new(2.0, 2);
        let fallback_price = Price::new(0.40, 2);
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                fallback_price,
                ClientOrderId::from("O-19700101-000000-001-013-1"),
            )
            .expect("TrailingStopMarket entry order with explicit trailing fields should build");

        let admission = strategy
            .submit_admission_request_from_order(
                &BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    BoltV3OrderIntentKind::Entry,
                    fallback_price.to_string(),
                    &order,
                ),
                &order,
            )
            .expect("TrailingStopMarket admission should derive from the instrument price ceiling");

        let OrderAny::TrailingStopMarket(order) = order else {
            panic!("TrailingStopMarket config should build an NT trailing-stop-market order");
        };
        assert_eq!(order.order_side(), OrderSide::Buy);
        assert_eq!(order.order_type(), OrderType::TrailingStopMarket);
        assert_eq!(order.time_in_force(), TimeInForce::Gtd);
        assert_eq!(order.price(), None);
        assert_eq!(order.trigger_price(), Some(Price::new(0.52, 2)));
        assert_eq!(order.activation_price(), None);
        assert_eq!(order.trigger_type(), Some(TriggerType::LastPrice));
        assert_eq!(order.trailing_offset(), Some(Decimal::new(25, 1)));
        assert_eq!(
            order.trailing_offset_type(),
            Some(TrailingOffsetType::BasisPoints)
        );
        assert_eq!(order.expire_time(), Some(expire_time));
        assert!(!order.is_post_only());
        assert!(!order.is_reduce_only());
        assert!(!order.is_quote_quantity());
        assert_eq!(
            admission.notional,
            Decimal::from_str("1.998").expect("expected decimal should parse"),
            "a market-style TrailingStopMarket entry must be valued at qty * the instrument price ceiling (2 * 0.999)"
        );

        let managed_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-STOP-EXIT"),
            quantity,
            0.450,
        );
        set_managed_position(
            &mut strategy,
            managed_position,
            ManagedPositionOrigin::StrategyEntry,
        );

        let exit_fallback_price = Price::new(0.45, 2);
        let exit_order = strategy
            .build_configured_exit_order(
                instrument_id,
                OrderSide::Sell,
                quantity,
                exit_fallback_price,
                ClientOrderId::from("O-19700101-000000-001-014-1"),
            )
            .expect("TrailingStopMarket exit order with explicit activation price should build");

        // A market-style (price-less) EXIT is valued at the instrument's
        // structural price CEILING (`max_price`) — the universally fail-closed
        // worst case for any side/intent — NOT at its reference/activation price.
        // Valuing it at the activation price (as the pre-A4 code did) was a
        // reference-price estimate, not a structural bound. This is the exit
        // counterpart of the entry ceiling valuation above and must run through
        // the strategy method that carries instrument context.
        let exit_admission = strategy
            .submit_admission_request_from_order(
                &BoltV3OrderIntentEvidence::from_compiled_order(
                    strategy.config.strategy_id.clone(),
                    BoltV3OrderIntentKind::Exit,
                    exit_fallback_price.to_string(),
                    &exit_order,
                ),
                &exit_order,
            )
            .expect("market-style exit admission should derive from the instrument price ceiling");

        let OrderAny::TrailingStopMarket(exit_order) = exit_order else {
            panic!("TrailingStopMarket exit config should build an NT trailing-stop-market order");
        };
        assert_eq!(exit_order.order_side(), OrderSide::Sell);
        assert_eq!(exit_order.order_type(), OrderType::TrailingStopMarket);
        assert_eq!(exit_order.time_in_force(), TimeInForce::Gtd);
        assert_eq!(exit_order.price(), None);
        assert_eq!(exit_order.trigger_price(), Some(Price::new(0.48, 2)));
        assert_eq!(exit_order.activation_price(), Some(Price::new(0.48, 2)));
        // The fixture instrument declares max_price = 0.999 (the production NT
        // Polymarket adapter's ceiling), so the market-style exit cap is valued
        // at the ceiling (0.999 * 2 = 1.998), strictly ABOVE the 0.48
        // activation-price estimate it replaces (0.96).
        assert_eq!(
            exit_admission.notional,
            Decimal::from_str("1.998").expect("expected decimal should parse"),
            "a market-style exit must be valued at qty * the instrument price ceiling (2 * 0.999)"
        );
        assert!(
            exit_admission.notional
                > Decimal::from_str("0.48").expect("expected decimal should parse")
                    * Decimal::from_str(quantity.to_string().trim())
                        .expect("quantity should parse"),
            "the ceiling valuation must bound strictly above the activation-price estimate it replaces"
        );
        assert_eq!(exit_order.trigger_type(), Some(TriggerType::MarkPrice));
        assert_eq!(exit_order.trailing_offset(), Some(Decimal::new(3, 0)));
        assert_eq!(
            exit_order.trailing_offset_type(),
            Some(TrailingOffsetType::Ticks)
        );
        assert_eq!(exit_order.expire_time(), Some(expire_time));
        assert!(!exit_order.is_post_only());
        assert!(!exit_order.is_reduce_only());
        assert!(!exit_order.is_quote_quantity());
    }

    #[test]
    fn trailing_stop_market_order_objects_use_nt_default_types_when_omitted() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        strategy.config.entry_order.order_type = OrderType::TrailingStopMarket;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.trigger_price = Some(0.52);
        strategy.config.entry_order.trailing_offset = Some(2.5);
        strategy.config.entry_order.is_post_only = false;
        let instrument_id = selected_entry_instrument(&strategy);

        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                Quantity::new(2.0, 2),
                Price::new(0.40, 2),
                ClientOrderId::from("O-19700101-000000-001-019-1"),
            )
            .expect("TrailingStopMarket should use NT defaults for omitted type fields");

        let OrderAny::TrailingStopMarket(order) = order else {
            panic!("TrailingStopMarket config should build an NT trailing-stop-market order");
        };
        assert_eq!(order.trigger_type(), Some(TriggerType::Default));
        assert_eq!(
            order.trailing_offset_type(),
            Some(TrailingOffsetType::Price)
        );
    }

    #[test]
    fn trailing_stop_market_rejects_required_nt_fields_before_factory() {
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(0.40, 2);

        let mut missing_offset =
            ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut missing_offset);
        missing_offset.config.entry_order.order_type = OrderType::TrailingStopMarket;
        missing_offset.config.entry_order.time_in_force = TimeInForce::Gtc;
        missing_offset.config.entry_order.trigger_price = Some(0.52);
        missing_offset.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
        missing_offset.config.entry_order.trailing_offset_type = Some(TrailingOffsetType::Price);
        let missing_offset_error = missing_offset
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-015-1"),
            )
            .expect_err("TrailingStopMarket without trailing_offset should fail before factory");
        assert!(
            missing_offset_error.to_string().contains("trailing_offset"),
            "{missing_offset_error}"
        );

        for trailing_offset in [0.0, -0.01] {
            let mut invalid_offset =
                ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
            let _cache = register_test_strategy(&mut invalid_offset);
            invalid_offset.config.entry_order.order_type = OrderType::TrailingStopMarket;
            invalid_offset.config.entry_order.time_in_force = TimeInForce::Gtc;
            invalid_offset.config.entry_order.trigger_price = Some(0.52);
            invalid_offset.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
            invalid_offset.config.entry_order.trailing_offset = Some(trailing_offset);
            invalid_offset.config.entry_order.trailing_offset_type =
                Some(TrailingOffsetType::Price);
            let invalid_offset_error = invalid_offset
                .build_configured_entry_order(
                    instrument_id,
                    OrderSide::Buy,
                    quantity,
                    price,
                    ClientOrderId::from("O-19700101-000000-001-016-1"),
                )
                .expect_err(
                    "TrailingStopMarket with non-positive trailing_offset should fail before factory",
                );
            assert!(
                invalid_offset_error.to_string().contains("trailing_offset"),
                "{invalid_offset_error}"
            );
        }

        let mut missing_trigger =
            ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut missing_trigger);
        missing_trigger.config.entry_order.order_type = OrderType::TrailingStopMarket;
        missing_trigger.config.entry_order.time_in_force = TimeInForce::Gtc;
        missing_trigger.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
        missing_trigger.config.entry_order.trailing_offset = Some(1.0);
        missing_trigger.config.entry_order.trailing_offset_type = Some(TrailingOffsetType::Price);
        let missing_trigger_error = missing_trigger
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-017-1"),
            )
            .expect_err(
                "TrailingStopMarket without trigger_price or activation_price should fail before factory",
            );
        assert!(
            missing_trigger_error.to_string().contains("trigger_price")
                && missing_trigger_error
                    .to_string()
                    .contains("activation_price"),
            "{missing_trigger_error}"
        );

        let mut is_post_only = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut is_post_only);
        is_post_only.config.entry_order.order_type = OrderType::TrailingStopMarket;
        is_post_only.config.entry_order.time_in_force = TimeInForce::Gtc;
        is_post_only.config.entry_order.trigger_price = Some(0.52);
        is_post_only.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
        is_post_only.config.entry_order.trailing_offset = Some(1.0);
        is_post_only.config.entry_order.trailing_offset_type = Some(TrailingOffsetType::Price);
        is_post_only.config.entry_order.is_post_only = true;
        let is_post_only_error = is_post_only
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-018-1"),
            )
            .expect_err("TrailingStopMarket post-only should fail before factory");
        assert!(
            is_post_only_error.to_string().contains("is_post_only"),
            "{is_post_only_error}"
        );
    }

    #[test]
    fn configured_order_build_rejects_nt_model_invalid_tif_before_factory() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let _cache = register_test_strategy(&mut strategy);
        let instrument_id = selected_entry_instrument(&strategy);
        let quantity = Quantity::new(1.0, 2);
        let price = Price::new(0.40, 2);

        strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
        let limit_error = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-003-1"),
            )
            .expect_err("limit GTD without expire_time should fail before NT factory");
        assert!(
            limit_error.to_string().contains("expire_time"),
            "{limit_error}"
        );

        strategy.config.entry_order.order_type = OrderType::Market;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
        let market_error = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-004-1"),
            )
            .expect_err("market GTD should fail before NT factory");
        assert!(
            market_error
                .to_string()
                .contains("GTD not supported for Market orders"),
            "{market_error}"
        );

        strategy.config.entry_order.time_in_force = TimeInForce::Fok;
        strategy.config.entry_order.expire_time_unix_nanos = Some(4_102_444_800_000_000_000_u64);
        let market_expiry_error = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-005-1"),
            )
            .expect_err("market expire_time should fail before NT factory");
        assert!(
            market_expiry_error
                .to_string()
                .contains("expire_time is not supported for Market orders"),
            "{market_expiry_error}"
        );

        strategy.config.entry_order.expire_time_unix_nanos = None;
        strategy.config.entry_order.order_type = OrderType::StopLimit;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
        strategy.config.entry_order.trigger_price = Some(0.52);
        let stop_limit_error = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-005-1"),
            )
            .expect_err("StopLimit GTD without expire_time should fail before NT factory");
        assert!(
            stop_limit_error.to_string().contains("expire_time"),
            "{stop_limit_error}"
        );

        strategy.config.entry_order.order_type = OrderType::MarketIfTouched;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
        strategy.config.entry_order.trigger_price = Some(0.52);
        let market_if_touched_error = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-007-1"),
            )
            .expect_err("MarketIfTouched GTD without expire_time should fail before NT factory");
        assert!(
            market_if_touched_error.to_string().contains("expire_time"),
            "{market_if_touched_error}"
        );

        strategy.config.entry_order.order_type = OrderType::LimitIfTouched;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
        strategy.config.entry_order.trigger_price = Some(0.39);
        let limit_if_touched_error = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-008-1"),
            )
            .expect_err("LimitIfTouched GTD without expire_time should fail before NT factory");
        assert!(
            limit_if_touched_error.to_string().contains("expire_time"),
            "{limit_if_touched_error}"
        );

        strategy.config.entry_order.order_type = OrderType::TrailingStopMarket;
        strategy.config.entry_order.time_in_force = TimeInForce::Gtd;
        strategy.config.entry_order.trigger_price = Some(0.52);
        strategy.config.entry_order.trigger_type = Some(TriggerType::LastPrice);
        strategy.config.entry_order.trailing_offset = Some(1.0);
        strategy.config.entry_order.trailing_offset_type = Some(TrailingOffsetType::Price);
        let trailing_stop_market_error = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                quantity,
                price,
                ClientOrderId::from("O-19700101-000000-001-009-1"),
            )
            .expect_err("TrailingStopMarket GTD without expire_time should fail before NT factory");
        assert!(
            trailing_stop_market_error
                .to_string()
                .contains("expire_time"),
            "{trailing_stop_market_error}"
        );
    }

    #[test]
    fn entry_book_impact_cap_uses_configured_sell_side_book() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let selected_side = selected_entry_side(&strategy);
        strategy.config.entry_order.side = "sell".to_string();
        strategy.config.entry_order.position_side = "short".to_string();
        strategy.config.exit_order.side = "buy".to_string();
        strategy.config.exit_order.position_side = "short".to_string();
        strategy.config.book_impact_cap_bps = 0;
        set_configured_books_depth(
            &mut strategy,
            &[
                (BookAction::Clear, OrderSide::Buy, 0.44, 7.0),
                (BookAction::Add, OrderSide::Buy, 0.44, 7.0),
                (BookAction::Add, OrderSide::Buy, 0.42, 100.0),
                (BookAction::Add, OrderSide::Sell, 0.60, 100.0),
            ],
        );

        assert_eq!(
            strategy.visible_book_notional_cap(selected_side),
            Some(3.08)
        );
    }

    #[test]
    fn post_only_entry_book_impact_cap_uses_passive_side_book() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        let selected_side = selected_entry_side(&strategy);
        strategy.config.entry_order.is_post_only = true;
        strategy.config.book_impact_cap_bps = 0;
        set_configured_books_depth(
            &mut strategy,
            &[
                (BookAction::Clear, OrderSide::Buy, 0.44, 7.0),
                (BookAction::Add, OrderSide::Buy, 0.44, 7.0),
                (BookAction::Add, OrderSide::Buy, 0.42, 100.0),
                (BookAction::Add, OrderSide::Sell, 0.60, 100.0),
            ],
        );

        assert_eq!(
            strategy.visible_book_notional_cap(selected_side),
            Some(3.08)
        );
    }

    #[test]
    fn configured_short_position_contract_is_rejected_until_short_economics_exists() {
        let contract = ConfiguredPositionContract {
            entry_order_side: OrderSide::Sell,
            entry_position_side: PositionSide::Short,
            exit_order_side: OrderSide::Buy,
            exit_position_side: PositionSide::Short,
        };

        assert!(!supports_strategy_managed_position(
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
        let instrument_id = selected_entry_instrument(&ready_to_trade_strategy());
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
        let mut loose = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        loose.config.book_impact_cap_bps = 5_000;
        let loose_instrument_id = selected_entry_instrument(&loose);
        loose.active.books.update_from_deltas(&book_deltas(
            loose_instrument_id,
            &[
                (BookAction::Add, OrderSide::Buy, 0.49, 10.0),
                (BookAction::Add, OrderSide::Sell, 0.50, 10.0),
                (BookAction::Add, OrderSide::Sell, 0.60, 10.0),
            ],
        ));

        let mut tight = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        tight.config.book_impact_cap_bps = 0;
        let tight_instrument_id = selected_entry_instrument(&tight);
        tight.active.books.update_from_deltas(&book_deltas(
            tight_instrument_id,
            &[
                (BookAction::Add, OrderSide::Buy, 0.49, 10.0),
                (BookAction::Add, OrderSide::Sell, 0.50, 10.0),
                (BookAction::Add, OrderSide::Sell, 0.60, 10.0),
            ],
        ));

        let loose_cap = loose.visible_book_notional_cap(selected_entry_side(&loose));
        let tight_cap = tight.visible_book_notional_cap(selected_entry_side(&tight));

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
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let instrument_a = pending.instrument_id;
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
        let tracked_instrument = selected_entry_instrument(&strategy);
        let exit_client_order_id = ClientOrderId::from("EXIT-A");
        let position_id = PositionId::from("P-A");
        let open_position = materialize_configured_position(
            &mut strategy,
            tracked_instrument,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        );
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
        let tracked_instrument = selected_entry_instrument(&strategy);
        let exit_client_order_id = ClientOrderId::from("EXIT-UNKNOWN");
        let position_id = PositionId::from("P-UNKNOWN");
        let mut open_position = materialize_configured_position(
            &mut strategy,
            tracked_instrument,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        );
        open_position.market_id = None;
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
        let tracked_instrument = selected_entry_instrument(&strategy);
        let exit_client_order_id = ClientOrderId::from("EXIT-DELAYED");
        let position_id = PositionId::from("P-DELAYED");
        let open_position = materialize_configured_position(
            &mut strategy,
            tracked_instrument,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        );
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
        let position_outcome_side = selected_entry_side(&strategy);
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
            outcome_side: Some(position_outcome_side),
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
        let entry_client_order_id = ClientOrderId::from("ENTRY-A");
        let position_id = PositionId::from("P-A");
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let instrument_a = pending.instrument_id;
        let original_book = pending.book.clone();
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
    fn maker_entry_partial_fills_keep_entry_fill_accounting_without_overwriting_position_event_quantity()
     {
        let mut strategy = ready_to_trade_strategy();
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.is_post_only = true;
        let entry_client_order_id = ClientOrderId::from("ENTRY-PARTIAL");
        let position_id = PositionId::from("P-PARTIAL");
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let instrument_id = pending.instrument_id;
        set_pending_entry(&mut strategy, pending);

        let mut first_fill = order_filled_event(entry_client_order_id, instrument_id, position_id);
        first_fill.last_qty = Quantity::new(4.0, 2);
        strategy
            .on_order_filled(&first_fill)
            .expect("first maker partial fill should be recorded");
        strategy.on_position_opened(position_opened_event(
            instrument_id,
            position_id,
            Quantity::new(4.0, 2),
            0.450,
        ));

        let mut second_fill = order_filled_event(entry_client_order_id, instrument_id, position_id);
        second_fill.last_qty = Quantity::new(6.0, 2);
        strategy
            .on_order_filled(&second_fill)
            .expect("later maker partial fill for same order should be recorded");

        assert_eq!(strategy.market_churn_count("MKT-1"), 2);
        assert_eq!(
            managed_position_ref(&strategy).map(|position| position.quantity),
            Some(Quantity::new(4.0, 2)),
            "OrderFilled carries last fill quantity; NT position events remain authoritative for aggregate position quantity"
        );
    }

    #[test]
    fn managed_partial_entry_blocks_normal_exit_until_entry_order_resolves() {
        let configured_instruments = configured_outcome_instruments(&ready_to_trade_strategy());
        for instrument_id in configured_instruments {
            let mut strategy = ready_to_trade_strategy();
            strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
            strategy.config.entry_order.is_post_only = true;
            strategy.active.phase = SelectionPhase::Active;
            let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
            materialize_managed_position_with_resting_pending_entry(
                &mut strategy,
                instrument_id,
                PositionId::from(format!("POSITION-NORMAL-WORKING-{instrument_id}").as_str()),
                position_quantity,
            );

            let decision = strategy.exit_submission_decision_at(1_200);

            assert_eq!(
                decision.blocked_reason,
                Some(EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING),
                "{instrument_id}"
            );
            assert_eq!(
                decision.evaluation.blocked_reason,
                Some(EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING),
                "{instrument_id}"
            );
            assert_eq!(decision.instrument_id, None, "{instrument_id}");
            assert_eq!(decision.order_side, None, "{instrument_id}");
            assert_eq!(decision.quantity, None, "{instrument_id}");
            assert!(decision.forced_flat_reasons.is_empty(), "{instrument_id}");
        }
    }

    #[test]
    fn forced_flat_exit_submits_despite_resting_pending_entry() {
        let configured_instruments = configured_outcome_instruments(
            &ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO),
        );
        for instrument_id in configured_instruments {
            let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
            strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
            strategy.config.entry_order.is_post_only = true;
            strategy.config.exit_order.order_type = OrderType::Limit;
            strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
            strategy.config.exit_order.is_post_only = true;
            strategy.active.phase = SelectionPhase::Freeze;
            let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
            let expected_exit_time_in_force = strategy.config.forced_exit_order.time_in_force;
            let expected_exit_reduce_only = strategy.config.forced_exit_order.is_reduce_only;
            materialize_managed_position_with_resting_pending_entry(
                &mut strategy,
                instrument_id,
                PositionId::from(format!("POSITION-FORCED-WORKING-{instrument_id}").as_str()),
                position_quantity,
            );
            let expected_exit_price = strategy
                .managed_position()
                .and_then(|managed| managed.position.book.best_bid);
            let expected_quantity = strategy
                .managed_position()
                .expect("fixture should materialize managed position")
                .position
                .quantity;

            let decision = strategy.exit_submission_decision_at(1_200);

            assert_eq!(decision.blocked_reason, None, "{instrument_id}");
            assert_eq!(decision.evaluation.blocked_reason, None, "{instrument_id}");
            assert_eq!(
                decision.forced_flat_reasons,
                vec![ForcedFlatReason::Freeze],
                "{instrument_id}"
            );
            assert_eq!(
                decision.order_type,
                Some(OrderType::Market),
                "{instrument_id}"
            );
            assert_eq!(
                decision.time_in_force,
                Some(expected_exit_time_in_force),
                "{instrument_id}"
            );
            assert_eq!(
                decision.order_side,
                Some(OrderSide::Sell),
                "{instrument_id}"
            );
            assert_eq!(
                decision.quantity,
                Some(expected_quantity),
                "{instrument_id}"
            );
            assert_eq!(decision.price, expected_exit_price, "{instrument_id}");
            assert_eq!(decision.is_post_only, Some(false), "{instrument_id}");
            assert_eq!(
                decision.is_reduce_only,
                Some(expected_exit_reduce_only),
                "{instrument_id}"
            );
        }
    }

    #[test]
    fn forced_flat_submit_cancels_resting_entry_and_recovers_if_entry_fill_races() {
        let configured_instruments = configured_outcome_instruments(
            &ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO),
        );
        for instrument_id in configured_instruments {
            let submit_admission = Arc::new(
                crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                    RecordingDecisionEvidenceWriter,
                )),
            );
            submit_admission
                .arm(live_canary_gate_report(1, Decimal::new(10_000, 0)))
                .expect("valid gate report should arm submit admission");
            let (mut strategy, fee_provider) =
                ready_to_trade_strategy_with_recording_fees(Decimal::ZERO, Decimal::ZERO);
            strategy.context = StrategyBuildContext::new(
                fee_provider,
                Arc::new(RecordingDecisionEvidenceWriter),
                submit_admission,
                fixture_execution_venue(),
            )
            .with_readiness_evidence(test_readiness_gate_evidence());
            strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
            strategy.config.entry_order.is_post_only = true;
            strategy.active.phase = SelectionPhase::Freeze;
            let cache = register_test_strategy(&mut strategy);
            add_active_instruments_to_cache(&strategy, &cache);
            let (risk_handler, risk_messages) =
                get_typed_into_message_saving_handler::<TradingCommand>(None);
            msgbus::register_trading_command_endpoint(
                MessagingSwitchboard::risk_engine_queue_execute(),
                risk_handler,
            );
            let (exec_handler, exec_messages) =
                get_typed_into_message_saving_handler::<TradingCommand>(None);
            msgbus::register_trading_command_endpoint(
                MessagingSwitchboard::exec_engine_queue_execute(),
                exec_handler,
            );
            let position_id =
                PositionId::from(format!("POSITION-FORCED-RACE-{instrument_id}").as_str());
            let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
            let (instrument_id, entry_client_order_id) =
                materialize_managed_position_with_resting_pending_entry(
                    &mut strategy,
                    instrument_id,
                    position_id,
                    position_quantity,
                );
            let entry_price = configured_book_for_instrument(&mut strategy, instrument_id)
                .best_ask
                .expect("ready-to-trade fixture should expose an ask");
            let entry_order = strategy
                .build_configured_entry_order(
                    instrument_id,
                    strategy
                        .configured_entry_order_side()
                        .expect("test config should carry entry order side"),
                    position_quantity,
                    Price::new(entry_price, 2),
                    entry_client_order_id,
                )
                .expect("resting entry order should build through NT factory");
            cache
                .borrow_mut()
                .add_order(
                    entry_order,
                    None,
                    Some(ClientId::from(strategy.config.client_id.as_str())),
                    true,
                )
                .expect("test cache should accept resting entry order");

            let exit_client_order_id = strategy
                .try_submit_exit_order(1_200)
                .expect("forced-flat exit submit should not fail")
                .expect("forced-flat exit should submit");

            let exec_messages = exec_messages.get_messages();
            assert!(
                exec_messages.iter().any(|message| matches!(
                    message,
                    TradingCommand::CancelOrder(command)
                        if command.client_order_id == entry_client_order_id
                )),
                "forced-flat submit should cancel the resting entry before relying on exit: {instrument_id}"
            );
            let risk_messages = risk_messages.get_messages();
            assert!(
                risk_messages.iter().any(|message| matches!(
                    message,
                    TradingCommand::SubmitOrder(command)
                        if command.client_order_id == exit_client_order_id
                )),
                "forced-flat exit should still submit after the entry cancel request: {instrument_id}"
            );

            strategy
                .on_order_filled(&order_filled_event(
                    entry_client_order_id,
                    instrument_id,
                    position_id,
                ))
                .expect("racing entry fill should be handled while exit is pending");
            strategy
                .on_order_filled(&order_filled_event_with_details(
                    exit_client_order_id,
                    instrument_id,
                    Some(position_id),
                    OrderSide::Sell,
                ))
                .expect("exit fill should be handled");
            strategy.on_order_expired(order_expired_event(exit_client_order_id, instrument_id));

            assert!(
                strategy.managed_position().is_some(),
                "entry remainder fill racing the first forced-flat exit should recover to managed residual exposure: {instrument_id}"
            );
            assert!(
                strategy.exposure.exit_pending().is_none(),
                "terminal forced-flat exit with residual exposure must not stay exit-pending forever: {instrument_id}"
            );
        }
    }

    #[test]
    fn non_resting_entry_fill_does_not_keep_pending_entry_from_cache_state() {
        let mut strategy = ready_to_trade_strategy();
        let cache = register_test_strategy(&mut strategy);
        strategy.config.entry_order.time_in_force = TimeInForce::Ioc;
        let entry_client_order_id = ClientOrderId::from("ENTRY-IOC");
        let position_id = PositionId::from("P-IOC");
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let instrument_id = pending.instrument_id;
        set_pending_entry(&mut strategy, pending);
        let order = strategy
            .build_configured_entry_order(
                instrument_id,
                OrderSide::Buy,
                Quantity::new(10.0, 2),
                Price::new(0.45, 2),
                entry_client_order_id,
            )
            .expect("IOC entry order should build");
        cache
            .borrow_mut()
            .add_order(order, None, Some(ClientId::from("POLYMARKET")), true)
            .expect("test cache should accept entry order");

        strategy
            .on_order_filled(&order_filled_event(
                entry_client_order_id,
                instrument_id,
                position_id,
            ))
            .expect("IOC entry fill should materialize a managed position");

        assert_eq!(
            strategy
                .managed_position()
                .and_then(|managed| managed.pending_entry.as_ref()),
            None
        );
        assert_eq!(
            strategy.exit_submission_decision_at(1_200).blocked_reason,
            None
        );
    }

    #[test]
    fn stop_market_exit_submission_uses_trigger_price_without_book_liquidity() {
        let mut strategy = ready_to_trade_strategy();
        strategy.config.exit_order.order_type = OrderType::StopMarket;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
        strategy.config.exit_order.trigger_price = Some(0.40);
        strategy.config.exit_order.trigger_type = Some(TriggerType::LastPrice);
        strategy.config.exit_order.is_post_only = false;
        let instrument_id = selected_entry_instrument(&strategy);
        let mut book = configured_book_for_instrument(&mut strategy, instrument_id);
        book.bid_levels.clear();
        book.ask_levels.clear();
        book.best_bid = None;
        book.best_ask = None;
        book.liquidity_available = Some(500.0);
        let mut position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-STOP-EXIT"),
            Quantity::new(4.0, 2),
            0.450,
        );
        position.book = book;
        set_managed_position(
            &mut strategy,
            position,
            ManagedPositionOrigin::StrategyEntry,
        );

        let decision = strategy.exit_submission_decision_at(1_200);

        assert_eq!(decision.blocked_reason, None);
        assert_eq!(decision.order_type, Some(OrderType::StopMarket));
        assert_eq!(decision.order_side, Some(OrderSide::Sell));
        assert_eq!(decision.price, Some(0.40));
        assert_eq!(decision.quantity, Some(Quantity::new(4.0, 2)));
    }

    #[test]
    fn stop_market_exit_ev_uses_trigger_price_instead_of_live_book() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.config.exit_order.order_type = OrderType::StopMarket;
        strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
        strategy.config.exit_order.trigger_price = Some(0.40);
        strategy.config.exit_order.trigger_type = Some(TriggerType::LastPrice);
        strategy.config.exit_order.is_post_only = false;
        let instrument_id = selected_entry_instrument(&strategy);
        let position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-STOP-EV"),
            Quantity::new(4.0, 2),
            0.450,
        );
        set_managed_position(
            &mut strategy,
            position,
            ManagedPositionOrigin::StrategyEntry,
        );

        let decision = strategy.exit_submission_decision_at(1_200);
        let exit_ev_bps = decision
            .evaluation
            .exit_ev_bps
            .unwrap_or_else(|| panic!("triggered exit EV should be available: {decision:#?}"));
        let expected_exit_ev_bps = ((0.40 - 0.450) / 0.450) * BPS_DENOMINATOR;

        assert!((exit_ev_bps - expected_exit_ev_bps).abs() < 1e-9);
    }

    #[test]
    fn entry_fill_without_position_id_stays_fail_closed_until_position_event_arrives() {
        let mut strategy = ready_to_trade_strategy();
        let entry_client_order_id = ClientOrderId::from("ENTRY-NO-POS");
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let instrument_id = pending.instrument_id;
        let original_book = pending.book.clone();
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

        let mut canceled = ready_to_trade_strategy();
        let canceled_pending = pending_entry_state(&mut canceled, entry_client_order_id);
        let instrument_id = canceled_pending.instrument_id;
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
        let rejected_pending = pending_entry_state(&mut rejected, entry_client_order_id);
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
        let expired_pending = pending_entry_state(&mut expired, entry_client_order_id);
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
        let rejecting_submit_admission = submit_admission_armed_with_cap(
            Decimal::new(1, 2),
            Arc::new(RecordingDecisionEvidenceWriter),
        );
        let mut direct = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
            Arc::new(RecordingDecisionEvidenceWriter),
            rejecting_submit_admission.clone(),
        );
        register_test_strategy_with_active_instruments(&mut direct);
        let direct_error = direct
            .try_submit_entry_order(1_200)
            .expect_err("test setup must reach submit-admission cap rejection");
        assert!(
            direct_error
                .to_string()
                .contains("notional cap is exceeded"),
            "test setup must prove submit-admission failure path: {direct_error:#}"
        );

        let rejecting_submit_admission = submit_admission_armed_with_cap(
            Decimal::new(1, 2),
            Arc::new(RecordingDecisionEvidenceWriter),
        );
        let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
            Arc::new(RecordingDecisionEvidenceWriter),
            rejecting_submit_admission,
        );
        register_test_strategy_with_active_instruments(&mut strategy);
        let instrument_id = selected_entry_instrument(&strategy);
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
        let rejecting_submit_admission = submit_admission_armed_with_cap(
            Decimal::new(1, 2),
            Arc::new(RecordingDecisionEvidenceWriter),
        );
        let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
            Arc::new(RecordingDecisionEvidenceWriter),
            rejecting_submit_admission.clone(),
        );
        strategy.active.phase = SelectionPhase::Freeze;
        let instrument_id = selected_entry_instrument(&strategy);
        let position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            PositionId::from("P-EXIT-SUBMIT-ERROR"),
            Quantity::new(10.0, 2),
            0.450,
        );
        set_managed_position(
            &mut strategy,
            position,
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
        let managed_position = strategy
            .managed_position()
            .expect("managed position should remain available for exit admission setup");
        let exit_order_side = decision
            .order_side
            .expect("exit decision should include order side");
        let exit_quantity = Decimal::from_f64(
            decision
                .quantity
                .expect("exit decision should include quantity")
                .as_f64(),
        )
        .expect("exit quantity should convert to decimal");
        let position_quantity = Decimal::from_f64(managed_position.position.quantity.as_f64())
            .expect("position quantity should convert to decimal");
        rejecting_submit_admission
            .admit(&BoltV3SubmitAdmissionRequest {
                strategy_id: strategy.config.strategy_id.clone(),
                client_order_id: "EXIT-SLOT-ALREADY-USED".to_string(),
                instrument_id: managed_position.position.instrument_id.to_string(),
                notional: Decimal::new(1, 0),
                order_side: exit_order_side,
                order_quantity: exit_quantity,
                intent_kind: BoltV3SubmitIntentKind::RiskReducingExit,
                lifecycle_policy: strategy.submit_lifecycle_policy(),
                canary_proof_claim: None,
                risk_reducing_exit_proof: Some(BoltV3RiskReducingExitProof {
                    position_id: managed_position.position.position_id.to_string(),
                    instrument_id: managed_position.position.instrument_id.to_string(),
                    position_side: managed_position.position.side,
                    exit_order_side,
                    position_quantity,
                    exit_quantity,
                }),
            })
            .expect("test setup should consume the only risk-reducing exit slot");

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
        let pending = pending_entry_state(
            &mut strategy,
            ClientOrderId::from("ENTRY-RECONCILE-BOOK-DELTA"),
        );
        let instrument_id = pending.instrument_id;
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
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let instrument_id = pending.instrument_id;
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
    fn position_closed_cancels_managed_resting_pending_entry_and_keeps_context() {
        let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
        strategy.config.entry_order.time_in_force = TimeInForce::Gtc;
        strategy.config.entry_order.is_post_only = true;
        let cache = register_test_strategy(&mut strategy);
        add_active_instruments_to_cache(&strategy, &cache);
        let (exec_handler, exec_messages) =
            get_typed_into_message_saving_handler::<TradingCommand>(None);
        msgbus::register_trading_command_endpoint(
            MessagingSwitchboard::exec_engine_queue_execute(),
            exec_handler,
        );
        let instrument_id = selected_entry_instrument(&strategy);
        let position_id = PositionId::from("POSITION-CLOSED-CANCELS-ENTRY");
        let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
        let (instrument_id, entry_client_order_id) =
            materialize_managed_position_with_resting_pending_entry(
                &mut strategy,
                instrument_id,
                position_id,
                position_quantity,
            );
        let entry_price = configured_book_for_instrument(&mut strategy, instrument_id)
            .best_ask
            .expect("ready-to-trade fixture should expose an ask");
        let entry_order = strategy
            .build_configured_entry_order(
                instrument_id,
                strategy
                    .configured_entry_order_side()
                    .expect("test config should carry entry order side"),
                position_quantity,
                Price::new(entry_price, 2),
                entry_client_order_id,
            )
            .expect("resting entry order should build through NT factory");
        cache
            .borrow_mut()
            .add_order(
                entry_order,
                None,
                Some(ClientId::from(strategy.config.client_id.as_str())),
                true,
            )
            .expect("test cache should accept resting entry order");

        strategy.on_position_closed(position_closed_event(instrument_id, position_id));

        let exec_messages = exec_messages.get_messages();
        assert!(
            exec_messages.iter().any(|message| matches!(
                message,
                TradingCommand::CancelOrder(command)
                    if command.client_order_id == entry_client_order_id
            )),
            "external position close should cancel the resting entry"
        );
        assert!(matches!(
            strategy.exposure,
            ExposureState::PendingEntry(PendingEntryState {
                client_order_id,
                ..
            }) if client_order_id == entry_client_order_id
        ));
        assert!(strategy.pending_entry().is_some());

        strategy
            .on_order_canceled(&order_canceled_event(entry_client_order_id, instrument_id))
            .expect("entry cancel should clear retained pending-entry context");
        assert!(matches!(strategy.exposure, ExposureState::Flat));
        assert!(strategy.pending_entry().is_none());
    }

    #[test]
    fn position_closed_keeps_entry_reconcile_pending_for_different_instrument() {
        let mut strategy = ready_to_trade_strategy();
        let entry_client_order_id = ClientOrderId::from("ENTRY-CLOSE-OTHER-INSTRUMENT");
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let pending_instrument_id = pending.instrument_id;
        let other_instrument_id = configured_instrument_except(&strategy, pending_instrument_id);
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
    fn position_closed_quarantines_foreign_venue_managed_position_id_collision() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&strategy);
        let position_id = PositionId::from("P-FOREIGN-CLOSE-MANAGED");
        materialize_configured_position(
            &mut strategy,
            instrument_id,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        );
        let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

        strategy.on_position_closed(position_closed_event(foreign_instrument_id, position_id));

        assert_foreign_venue_blind_recovery(&strategy);
    }

    #[test]
    fn position_closed_quarantines_foreign_venue_exit_pending_position_id_collision() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = selected_entry_instrument(&strategy);
        let position_id = PositionId::from("P-FOREIGN-CLOSE-EXIT");
        let open_position = materialize_configured_position(
            &mut strategy,
            instrument_id,
            position_id,
            Quantity::new(10.0, 2),
            0.450,
        );
        set_exit_pending(
            &mut strategy,
            open_position,
            ClientOrderId::from("EXIT-FOREIGN-CLOSE"),
            false,
            false,
            ManagedPositionOrigin::StrategyEntry,
        );
        let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

        strategy.on_position_closed(position_closed_event(foreign_instrument_id, position_id));

        assert_foreign_venue_blind_recovery(&strategy);
    }

    #[test]
    fn position_closed_quarantines_foreign_venue_unsupported_position_id_collision() {
        let mut strategy = ready_to_trade_strategy();
        let instrument_id = strategy.active.books.up.instrument_id.unwrap();
        let position_id = PositionId::from("P-FOREIGN-CLOSE-UNSUPPORTED");
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
        let foreign_instrument_id = foreign_venue_instrument_id(&strategy, instrument_id);

        strategy.on_position_closed(position_closed_event(foreign_instrument_id, position_id));

        assert_foreign_venue_blind_recovery(&strategy);
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
        let entry_client_order_id = ClientOrderId::from("ENTRY-SELL");
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let instrument_id = pending.instrument_id;
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
        let entry_client_order_id = ClientOrderId::from("ENTRY-MISMATCHED-FILL");
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let pending_instrument_id = pending.instrument_id;
        let fill_instrument_id = configured_instrument_except(&strategy, pending_instrument_id);
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
        let entry_client_order_id = ClientOrderId::from("ENTRY-SELL");
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let instrument_id = pending.instrument_id;
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
    fn live_position_event_quarantines_foreign_venue_position() {
        // P5-5 / Codex P5 — LIVE-PATH regression lock (mirror of
        // `recovery_bootstrap_quarantines_foreign_venue_position`). The recovery path already
        // quarantines a foreign-venue cached position before its contract check; the LIVE
        // position-event path (`materialize_position_from_event`, driven by `on_position_opened`)
        // must do the same. This event carries a SUPPORTED side/contract shape (Buy / Long, the
        // exact shape the same-venue baseline `position_events_update_live_position_state` adopts
        // into Managed), so the ONLY thing making it foreign is the venue. Under the pre-guard
        // code this foreign event would pass the side + contract checks and become Managed (the
        // exit path then submits a real order against that foreign instrument_id); the new venue
        // guard is what diverts it to blind recovery instead.
        let mut strategy = ready_to_trade_strategy();
        let execution_venue = strategy.context.execution_venue();
        let execution_instrument = configured_outcome_instruments(&strategy)
            .into_iter()
            .next()
            .expect("ready-to-trade fixture should expose a configured instrument");
        assert_eq!(
            execution_instrument.venue, execution_venue,
            "fixture instrument must be on the execution venue so only the venue is changed",
        );
        // Same symbol, foreign venue: the venue is the ONLY difference from a managed position.
        let foreign_instrument =
            InstrumentId::new(execution_instrument.symbol, Venue::from("HYPERLIQUID"));
        assert_ne!(
            foreign_instrument.venue, execution_venue,
            "foreign instrument must be on a non-execution venue",
        );

        strategy.on_position_opened(position_opened_event_with_details(
            foreign_instrument,
            PositionId::from("P-FOREIGN-LIVE"),
            Quantity::new(10.0, 2),
            0.450,
            OrderSide::Buy,
            PositionSide::Long,
        ));

        // Observable exposure: quarantined to blind recovery, never adopted into Managed.
        assert!(
            matches!(
                strategy.exposure,
                ExposureState::BlindRecovery(BlindRecoveryState {
                    reason: BlindRecoveryReason::ForeignVenuePosition { .. }
                })
            ),
            "foreign-venue live position event must be quarantined to blind recovery, got {:?}",
            strategy.exposure,
        );
        assert!(strategy.managed_position().is_none());
    }

    #[test]
    fn order_fill_entry_quarantines_foreign_venue_position() {
        // P5-5 / Codex P5 — LIVE ORDER-FILL regression lock (sibling of
        // `live_position_event_quarantines_foreign_venue_position`). The entry-fill branch of
        // `on_order_filled` matches a fill to our pending entry by client_order_id ALONE, then
        // adopts the fill's instrument_id into Managed (origin StrategyEntry). A foreign-venue fill
        // that happens to carry our pending entry's client_order_id must NOT be adopted — the exit
        // path would otherwise submit a real order against the foreign instrument_id. The shared
        // venue-adoption guard must divert it to blind recovery, exactly as the position-event path
        // does. Pre-guard this fill (Some position_id + supported Buy/Long side) would become Managed.
        let mut strategy = ready_to_trade_strategy();
        let execution_venue = strategy.context.execution_venue();
        let entry_client_order_id = ClientOrderId::from("ENTRY-FOREIGN-FILL");
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let execution_instrument_id = pending.instrument_id;
        assert_eq!(
            execution_instrument_id.venue, execution_venue,
            "pending entry must be on the execution venue so only the fill venue differs",
        );
        set_pending_entry(&mut strategy, pending);

        // Same symbol, foreign venue: the venue is the ONLY difference from our pending entry.
        let foreign_instrument_id =
            InstrumentId::new(execution_instrument_id.symbol, Venue::from("HYPERLIQUID"));
        assert_ne!(
            foreign_instrument_id.venue, execution_venue,
            "foreign fill must be on a non-execution venue",
        );

        strategy
            .on_order_filled(&order_filled_event_with_details(
                entry_client_order_id,
                foreign_instrument_id,
                Some(PositionId::from("P-FOREIGN-FILL")),
                OrderSide::Buy,
            ))
            .expect("foreign-venue entry fill must not wedge the strategy");

        // Observable exposure: quarantined to blind recovery, never adopted into Managed.
        assert!(
            matches!(
                strategy.exposure,
                ExposureState::BlindRecovery(BlindRecoveryState {
                    reason: BlindRecoveryReason::ForeignVenuePosition { .. }
                })
            ),
            "foreign-venue entry fill must be quarantined to blind recovery, got {:?}",
            strategy.exposure,
        );
        assert!(strategy.managed_position().is_none());
    }

    #[test]
    fn pending_entry_unknown_position_side_stays_fail_closed_without_materializing_position() {
        let mut strategy = ready_to_trade_strategy();
        let entry_client_order_id = ClientOrderId::from("ENTRY-BAD-SIDE");
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let instrument_id = pending.instrument_id;
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
        let instrument_a = selected_entry_instrument(&strategy);
        let preserved_book = configured_book_for_instrument(&mut strategy, instrument_a);
        let preserved_position = materialize_configured_position(
            &mut strategy,
            instrument_a,
            PositionId::from("P-A"),
            Quantity::new(10.0, 2),
            0.450,
        );
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
    fn interval_open_captures_source_bound_price_to_beat_at_or_after_market_start() {
        let mut strategy = test_strategy();
        let mut snapshot = active_snapshot_with_start("MKT-1", 1_000);
        let SelectionState::Active { market } = &mut snapshot.decision.state else {
            panic!("expected active snapshot");
        };
        market.price_to_beat = Some(3_099.0);
        strategy.apply_selection_snapshot(snapshot);

        strategy.observe_reference_snapshot(&reference_tick(900, 3_100.0));
        assert!(strategy.active.interval_open.is_none());

        strategy.observe_reference_snapshot(&reference_tick(1_000, 3_101.0));
        assert_eq!(strategy.active.interval_open, Some(3_099.0));
    }

    #[test]
    fn interval_open_requires_source_bound_price_to_beat_before_reference_quote_warms_market() {
        let mut strategy = test_strategy();
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

        strategy.observe_reference_quote(&fast_spot("reference", 3_101.0, 1_000));

        assert_eq!(strategy.active.interval_open, None);
        assert_eq!(strategy.active.last_reference_ts_ms, None);
        assert_eq!(strategy.active.warmup_count, INITIAL_COUNTER_U64);
    }

    #[test]
    fn interval_open_does_not_use_reference_price_without_source_bound_price_to_beat() {
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

        assert_eq!(strategy.active.interval_open, None);
        assert_eq!(strategy.active.last_reference_ts_ms, None);
        assert_eq!(strategy.active.warmup_count, INITIAL_COUNTER_U64);
    }

    #[test]
    fn interval_open_uses_source_bound_price_to_beat_over_reference() {
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
    fn interval_open_does_not_use_fused_reference_without_source_bound_price_to_beat() {
        let mut strategy = test_strategy();
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-1", 1_000));

        strategy.observe_reference_snapshot(&ReferenceSnapshot {
            ts_ms: 1_000,
            topic: "platform.reference.test.spot".to_string(),
            fair_value: Some(3_107.0),
            confidence: 1.0,
            venues: vec![],
        });

        assert_eq!(strategy.active.interval_open, None);
        assert_eq!(strategy.active.last_reference_ts_ms, None);
        assert_eq!(strategy.active.warmup_count, INITIAL_COUNTER_U64);
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

    #[test]
    fn entry_evaluation_uses_price_adjusted_fee_bps_not_cached_base_fee_rate() {
        let (mut strategy, fee_provider) =
            ready_to_trade_strategy_with_recording_fees(Decimal::from(1000), Decimal::from(1000));
        fee_provider.set_entry_fee_bps(
            "condition-MKT-1-MKT-1-UP.POLYMARKET",
            Decimal::from_str("511.111111111111").expect("test decimal should parse"),
        );
        fee_provider.set_entry_fee_bps(
            "condition-MKT-1-MKT-1-DOWN.POLYMARKET",
            Decimal::from_str("182.027027027027").expect("test decimal should parse"),
        );
        register_test_strategy_with_active_instruments(&mut strategy);

        let decision = strategy.entry_submission_decision_at(1_200);
        let fields = strategy.entry_evaluation_log_fields_at(1_200, &decision);

        assert_eq!(fields.up_fee_bps, Some(511.111111111111));
        assert_eq!(fields.down_fee_bps, Some(182.027027027027));
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
    fn book_delta_refreshes_fee_readiness_after_warm_populates_provider() {
        let fee_provider = RecordingFeeProvider::cold();
        let mut strategy = ready_to_trade_strategy();
        strategy.context = StrategyBuildContext::new(
            fee_provider.clone(),
            Arc::new(RecordingDecisionEvidenceWriter),
            Arc::new(
                crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
                    RecordingDecisionEvidenceWriter,
                )),
            ),
            fixture_execution_venue(),
        )
        .with_readiness_evidence(test_readiness_gate_evidence());
        strategy.active.outcome_fees.up_ready = false;
        strategy.active.outcome_fees.down_ready = false;
        register_test_strategy_with_active_instruments(&mut strategy);

        let up_instrument_id = strategy
            .active
            .outcome_fees
            .up_instrument_id
            .expect("test active market should have up outcome");
        let down_instrument_id = strategy
            .active
            .outcome_fees
            .down_instrument_id
            .expect("test active market should have down outcome");
        fee_provider.set_fee(up_instrument_id.to_string().as_str(), Decimal::new(100, 2));
        fee_provider.set_fee(
            down_instrument_id.to_string().as_str(),
            Decimal::new(100, 2),
        );

        strategy
            .on_book_deltas(&book_deltas(
                up_instrument_id,
                &[(BookAction::Update, OrderSide::Sell, 0.45, 500.0)],
            ))
            .expect("book delta should not escape actor loop");

        assert!(strategy.active.outcome_fees.market_ready());
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
        let mut snapshot = freeze_snapshot_with_start("MKT-1", 1_000);
        let SelectionState::Freeze { market, .. } = &mut snapshot.decision.state else {
            panic!("expected freeze snapshot");
        };
        market.price_to_beat = Some(3_100.0);
        strategy.apply_selection_snapshot(snapshot);

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
        let gate = strategy.entry_gate_decision_at(1_100);
        assert!(
            gate.blocked_by
                .contains(&EntryBlockReason::ForcedFlat(ForcedFlatReason::Freeze))
        );
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
    fn signed_trade_flow_observe_appends_signed_price_and_size() {
        let mut config = test_strategy().config.clone();
        config.trade_flow_window_secs = 30;
        config.trade_flow_max_samples = 100;
        let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&config));

        flow.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.42,
            3.0,
            AggressorSide::Buyer,
            1_000,
        ));
        flow.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.41,
            2.0,
            AggressorSide::Seller,
            1_500,
        ));
        flow.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.40,
            1.0,
            AggressorSide::NoAggressor,
            2_000,
        ));

        assert_eq!(flow.len(), 3);
        assert!(!flow.is_empty());
        let samples: Vec<SignedTrade> = flow.samples().iter().copied().collect();
        // Prices/sizes are compared through the same fixed-point round-trip the
        // production path uses, so the test does not depend on literal f64 bits.
        assert_eq!(
            samples,
            vec![
                SignedTrade {
                    ts_ms: 1_000,
                    aggressor: AggressorSide::Buyer,
                    price: Price::new(0.42, 2).as_f64(),
                    size: Quantity::new(3.0, 0).as_f64(),
                },
                SignedTrade {
                    ts_ms: 1_500,
                    aggressor: AggressorSide::Seller,
                    price: Price::new(0.41, 2).as_f64(),
                    size: Quantity::new(2.0, 0).as_f64(),
                },
                SignedTrade {
                    ts_ms: 2_000,
                    aggressor: AggressorSide::NoAggressor,
                    price: Price::new(0.40, 2).as_f64(),
                    size: Quantity::new(1.0, 0).as_f64(),
                },
            ]
        );
    }

    #[test]
    fn signed_trade_flow_drops_out_of_order_and_duplicate_timestamps() {
        // The buffer doc promises samples "oldest first" and that it mirrors
        // RealizedVolEstimator, which rejects non-monotonic observations. A trade
        // whose timestamp is not strictly greater than the latest retained sample
        // would otherwise corrupt ordering and the time-window eviction cutoff, so
        // it must be dropped.
        let mut config = test_strategy().config.clone();
        config.trade_flow_window_secs = 600;
        config.trade_flow_max_samples = 100;
        let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&config));

        flow.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.50,
            1.0,
            AggressorSide::Buyer,
            1_000,
        ));
        flow.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.51,
            1.0,
            AggressorSide::Buyer,
            2_000,
        ));
        // Out-of-order: an earlier timestamp than the latest retained sample.
        flow.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.52,
            1.0,
            AggressorSide::Seller,
            1_500,
        ));
        // Duplicate: equal to the latest retained timestamp.
        flow.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.53,
            1.0,
            AggressorSide::Seller,
            2_000,
        ));

        assert_eq!(flow.len(), 2);
        assert_eq!(
            flow.samples()
                .iter()
                .map(|trade| trade.ts_ms)
                .collect::<Vec<_>>(),
            vec![1_000, 2_000],
            "out-of-order and duplicate-timestamp trades must be dropped to keep \
             the buffer monotonic"
        );
    }

    #[test]
    fn signed_trade_flow_samples_within_excludes_trades_aged_out_by_caller_clock() {
        // Eviction only runs inside `observe`, so in a quiet market `samples()`
        // can still hold trades that have aged out of the window. A point-in-time
        // consumer reads through `samples_within(now)`, which filters against the
        // caller's clock.
        let mut config = test_strategy().config.clone();
        config.trade_flow_window_secs = 10; // 10_000ms window
        config.trade_flow_max_samples = 100;
        let mut flow = SignedTradeFlow::from_config(&signed_trade_flow_config(&config));

        flow.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.50,
            1.0,
            AggressorSide::Buyer,
            1_000,
        ));
        flow.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.51,
            1.0,
            AggressorSide::Buyer,
            5_000,
        ));

        // observe-time eviction at 5_000ms (cutoff 5_000 - 10_000 saturates to 0)
        // retains both; the raw buffer is not filtered by the caller's clock.
        assert_eq!(flow.len(), 2);

        // As of now = 20_000ms the window is [10_000, 20_000]; both trades have
        // aged out, so a point-in-time read reports none.
        assert!(flow.samples_within(20_000).next().is_none());

        // As of now = 12_000ms the window is [2_000, 12_000]: the 5_000ms trade is
        // in-window, the 1_000ms trade has aged out.
        assert_eq!(
            flow.samples_within(12_000)
                .map(|trade| trade.ts_ms)
                .collect::<Vec<_>>(),
            vec![5_000]
        );
    }

    #[test]
    fn signed_trade_flow_evicts_by_window_then_caps_by_max_samples() {
        // Window eviction: a 10-second window drops trades older than now - window.
        let mut window_config = test_strategy().config.clone();
        window_config.trade_flow_window_secs = 10;
        window_config.trade_flow_max_samples = 100;
        let mut windowed = SignedTradeFlow::from_config(&signed_trade_flow_config(&window_config));

        windowed.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.50,
            1.0,
            AggressorSide::Buyer,
            1_000,
        ));
        windowed.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.51,
            1.0,
            AggressorSide::Buyer,
            5_000,
        ));
        // Latest trade at 15_000ms makes the window cutoff 5_000ms; the 1_000ms
        // sample is strictly older than the cutoff and is evicted, the 5_000ms
        // sample sits exactly on the cutoff and is retained.
        windowed.observe(&trade_tick_with_aggressor(
            "condition-A-A-UP.POLYMARKET",
            0.52,
            1.0,
            AggressorSide::Seller,
            15_000,
        ));

        assert_eq!(windowed.len(), 2);
        assert_eq!(
            windowed
                .samples()
                .iter()
                .map(|trade| trade.ts_ms)
                .collect::<Vec<_>>(),
            vec![5_000, 15_000]
        );

        // Count cap: with a wide window, max_samples bounds retained trades and
        // drops the oldest first.
        let mut cap_config = test_strategy().config.clone();
        cap_config.trade_flow_window_secs = 600;
        cap_config.trade_flow_max_samples = 2;
        let mut capped = SignedTradeFlow::from_config(&signed_trade_flow_config(&cap_config));

        for index in 0..4_u64 {
            capped.observe(&trade_tick_with_aggressor(
                "condition-A-A-UP.POLYMARKET",
                0.50,
                1.0,
                AggressorSide::Buyer,
                1_000 + index,
            ));
        }

        assert_eq!(capped.len(), 2);
        assert_eq!(
            capped
                .samples()
                .iter()
                .map(|trade| trade.ts_ms)
                .collect::<Vec<_>>(),
            vec![1_002, 1_003]
        );
    }

    #[test]
    fn on_trade_routes_to_subscribed_instrument_and_ignores_untracked() {
        let mut strategy = test_strategy();
        strategy.apply_selection_snapshot(active_snapshot("A"));

        let up_instrument = "condition-A-A-UP.POLYMARKET";
        let down_instrument = "condition-A-A-DOWN.POLYMARKET";
        let untracked_instrument = "condition-Z-Z-UP.POLYMARKET";

        strategy
            .on_trade(&trade_tick_with_aggressor(
                up_instrument,
                0.42,
                2.0,
                AggressorSide::Buyer,
                1_000,
            ))
            .expect("trade on subscribed instrument should process");
        strategy
            .on_trade(&trade_tick_with_aggressor(
                untracked_instrument,
                0.99,
                1.0,
                AggressorSide::Seller,
                1_100,
            ))
            .expect("trade on untracked instrument should be ignored without error");

        let up_flow = strategy
            .active
            .trade_flow
            .get(&InstrumentId::from(up_instrument))
            .expect("subscribed up instrument should have a trade-flow buffer");
        assert_eq!(up_flow.len(), 1);
        assert_eq!(up_flow.samples()[0].aggressor, AggressorSide::Buyer);

        let down_flow = strategy
            .active
            .trade_flow
            .get(&InstrumentId::from(down_instrument))
            .expect("subscribed down instrument should have a trade-flow buffer");
        assert!(down_flow.is_empty());

        assert!(
            !strategy
                .active
                .trade_flow
                .contains_key(&InstrumentId::from(untracked_instrument)),
            "untracked instrument must not create a trade-flow buffer"
        );
    }

    #[test]
    fn market_switch_creates_and_removes_trade_flow_buffers_in_lockstep_with_books() {
        let mut strategy = test_strategy();

        strategy.apply_selection_snapshot(active_snapshot("A"));
        assert_eq!(
            strategy.active.trade_flow.keys().collect::<Vec<_>>(),
            vec![
                &InstrumentId::from("condition-A-A-DOWN.POLYMARKET"),
                &InstrumentId::from("condition-A-A-UP.POLYMARKET"),
            ]
        );

        strategy.apply_selection_snapshot(active_snapshot("B"));
        assert_eq!(
            strategy.active.trade_flow.keys().collect::<Vec<_>>(),
            vec![
                &InstrumentId::from("condition-B-B-DOWN.POLYMARKET"),
                &InstrumentId::from("condition-B-B-UP.POLYMARKET"),
            ]
        );
    }

    #[test]
    fn same_market_refresh_preserves_accumulated_trade_flow() {
        let mut strategy = test_strategy();
        strategy.apply_selection_snapshot(active_snapshot_with_start("A", 1_000));

        strategy
            .on_trade(&trade_tick_with_aggressor(
                "condition-A-A-UP.POLYMARKET",
                0.42,
                2.0,
                AggressorSide::Buyer,
                1_200,
            ))
            .expect("trade on subscribed instrument should process");

        // A new interval for the same market must not discard accumulated flow.
        strategy.apply_selection_snapshot(active_snapshot_with_start("A", 2_000));

        let up_flow = strategy
            .active
            .trade_flow
            .get(&InstrumentId::from("condition-A-A-UP.POLYMARKET"))
            .expect("same-market refresh should retain the trade-flow buffer");
        assert_eq!(up_flow.len(), 1);
    }

    #[test]
    fn real_market_change_preserves_retained_instrument_trade_flow() {
        // Regression: on a real market change (preserve_books == false), the
        // trade_flow restore must be unconditional. An instrument that is
        // RETAINED across the rotation (here, the open position's instrument,
        // tracked via `tracked_position_instrument_id`) is touched by neither
        // `unsubscribe_missing_books` (it did not change away) nor
        // `subscribe_new_books` (it is not new), so dropping its buffer in
        // `apply_selection_snapshot_to_active` would silently lose all
        // accumulated SignedTradeFlow for the live position.
        let mut strategy = ready_to_trade_strategy();
        let entry_client_order_id = ClientOrderId::from("ENTRY-A");
        let position_id = PositionId::from("P-A");
        let pending = pending_entry_state(&mut strategy, entry_client_order_id);
        let retained_instrument = pending.instrument_id;
        set_pending_entry(&mut strategy, pending);

        // Rotate to a different market, then fill: the position materializes on
        // the original (now non-active) instrument, which becomes the tracked
        // position instrument with its own freshly subscribed trade-flow buffer.
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-2", 2_000));
        strategy
            .on_order_filled(&order_filled_event(
                entry_client_order_id,
                retained_instrument,
                position_id,
            ))
            .expect("fill bookkeeping should succeed");
        assert_eq!(
            strategy.book_subscriptions.tracked_position_instrument_id,
            Some(retained_instrument),
            "open position instrument must be the retained tracked instrument",
        );
        assert!(
            strategy
                .active
                .trade_flow
                .contains_key(&retained_instrument),
            "fill must subscribe a trade-flow buffer for the tracked instrument",
        );

        // Accumulate signed trade flow on the retained instrument.
        strategy
            .on_trade(&trade_tick_with_aggressor(
                retained_instrument.to_string().as_str(),
                0.42,
                2.0,
                AggressorSide::Buyer,
                2_200,
            ))
            .expect("trade on the tracked instrument should process");
        assert!(
            !strategy
                .active
                .trade_flow
                .get(&retained_instrument)
                .expect("tracked instrument buffer must exist before rotation")
                .is_empty(),
            "seeded trade should be retained before the rotation",
        );

        // Real market change (preserve_books == false). The retained instrument
        // is unchanged across the rotation, so its buffer must survive.
        strategy.apply_selection_snapshot(active_snapshot_with_start("MKT-3", 3_000));
        assert_eq!(
            strategy.book_subscriptions.tracked_position_instrument_id,
            Some(retained_instrument),
            "instrument should remain the tracked instrument across the rotation",
        );
        let retained_flow = strategy
            .active
            .trade_flow
            .get(&retained_instrument)
            .expect("retained instrument trade-flow buffer must survive a real market change");
        assert!(
            !retained_flow.is_empty(),
            "retained instrument must keep its accumulated SignedTradeFlow across a real market change",
        );
        assert_eq!(retained_flow.len(), 1);
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
    fn strategy_refuses_foreign_venue_market_even_when_slug_matches_the_target() {
        // P5-5 / Codex P5: the shared NT cache can hold instruments from venues OTHER than the
        // execution venue. A foreign-venue binary option that happens to carry the SAME updown slug
        // as the configured target must never be tradeable — a real order only ever routes to the
        // execution client's venue. The market matcher is venue-agnostic (it matches on slug +
        // outcome), so without venue scoping a colliding foreign instrument WOULD be selectable;
        // the execution-venue read filter excludes it and the explicit guard fails closed.
        let strategy = test_strategy();
        let current_start = 1_746_000_000_i64;
        let now_ms = current_start as u64 * MILLIS_PER_SECOND_U64 + 1;
        let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
            &strategy.config.underlying_asset,
            &strategy.config.cadence_slug_token,
            current_start,
        );
        let start_ms = current_start as u64 * MILLIS_PER_SECOND_U64;
        let end_ms = start_ms + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64;
        let execution_venue = fixture_execution_venue();

        // The SAME slug + market id exists on a NON-execution venue (a HIP-4 / Hyperliquid market
        // here) as well as on the Polymarket execution venue.
        let foreign = vec![
            updown_binary_option(
                "token-up.HYPERLIQUID",
                &market_slug,
                "market-1",
                "Up",
                start_ms,
                end_ms,
            ),
            updown_binary_option(
                "token-down.HYPERLIQUID",
                &market_slug,
                "market-1",
                "Down",
                start_ms,
                end_ms,
            ),
        ];
        let polymarket = vec![
            updown_binary_option(
                "token-up.POLYMARKET",
                &market_slug,
                "market-1",
                "Up",
                start_ms,
                end_ms,
            ),
            updown_binary_option(
                "token-down.POLYMARKET",
                &market_slug,
                "market-1",
                "Down",
                start_ms,
                end_ms,
            ),
        ];

        // The slug genuinely matches on the foreign venue too: in isolation the matcher selects it,
        // which is exactly the latent hazard.
        let foreign_snapshot =
            selection_snapshot_from_instruments(&strategy.config, &foreign, now_ms);
        let SelectionState::Active { market } = &foreign_snapshot.decision.state else {
            panic!(
                "foreign-venue instruments share the target slug and must select in isolation: {foreign_snapshot:?}"
            );
        };
        assert_eq!(market.up.instrument_id, "token-up.HYPERLIQUID");
        // ...but the execution-venue guard refuses that foreign selection (fail closed).
        assert!(
            !selected_market_on_execution_venue(&foreign_snapshot, execution_venue),
            "a selected market whose outcomes are on a non-execution venue must be refused",
        );

        // The execution-venue market selects and is accepted by the guard.
        let polymarket_snapshot =
            selection_snapshot_from_instruments(&strategy.config, &polymarket, now_ms);
        assert!(
            selected_market_on_execution_venue(&polymarket_snapshot, execution_venue),
            "the execution-venue market must pass the guard",
        );

        // The production cache read scopes by the execution venue, so from a MIXED cache only the
        // execution-venue market is ever considered for selection.
        let mixed = [foreign.clone(), polymarket].concat();
        let scoped = mixed
            .iter()
            .filter(|instrument| instrument.id().venue == execution_venue)
            .cloned()
            .collect::<Vec<_>>();
        let scoped_snapshot =
            selection_snapshot_from_instruments(&strategy.config, &scoped, now_ms);
        let SelectionState::Active { market } = scoped_snapshot.decision.state else {
            panic!(
                "execution-venue-scoped selection should still find the market: {scoped_snapshot:?}"
            );
        };
        assert_eq!(market.up.instrument_id, "token-up.POLYMARKET");
        assert_eq!(market.down.instrument_id, "token-down.POLYMARKET");
    }

    #[test]
    fn recovery_bootstrap_quarantines_foreign_venue_position() {
        // P5-5 / Codex P5 — RECOVERY-PATH regression lock. The entry path is venue-scoped, but
        // recovery bootstrap previously adopted any-venue cached positions, and the exit path would
        // then build/submit a real order on the foreign-venue instrument. `bootstrapped_exposure_for`
        // is the single fail-closed adoption decision and must quarantine a foreign-venue position
        // BEFORE the contract check. This test holds the venue as the ONLY difference between a managed
        // and a quarantined position, proving the venue guard is what diverts it.
        let mut strategy = ready_to_trade_strategy();
        let execution_venue = fixture_execution_venue();
        let instrument_id = configured_outcome_instruments(&strategy)
            .into_iter()
            .next()
            .expect("ready-to-trade fixture should expose a configured instrument");
        let supported = configured_position_probe(&mut strategy, instrument_id);
        assert_eq!(
            supported.instrument_id.venue, execution_venue,
            "probe should produce an execution-venue position",
        );

        // Control: an execution-venue, supported-side position is adopted into Managed.
        let managed = strategy.bootstrapped_exposure_for(supported.clone(), execution_venue);
        assert!(
            matches!(managed, ExposureState::Managed(_)),
            "execution-venue supported position must be managed, got {managed:?}",
        );

        // Same position on a foreign venue (only the venue differs) must be quarantined, never managed.
        let foreign_instrument =
            InstrumentId::new(supported.instrument_id.symbol, Venue::from("HYPERLIQUID"));
        let foreign = OpenPositionState {
            instrument_id: foreign_instrument,
            book: OutcomeBookState::from_instrument_id(foreign_instrument),
            ..supported.clone()
        };
        let quarantined = strategy.bootstrapped_exposure_for(foreign, execution_venue);
        assert!(
            matches!(
                quarantined,
                ExposureState::BlindRecovery(BlindRecoveryState {
                    reason: BlindRecoveryReason::ForeignVenuePosition { .. }
                })
            ),
            "foreign-venue position must be quarantined to blind recovery, got {quarantined:?}",
        );
    }

    #[test]
    fn refresh_selection_from_cache_filters_foreign_venue_in_production_path() {
        // P5-5 / Codex P5 — PRODUCTION-PATH regression lock. The sibling test
        // `strategy_refuses_foreign_venue_market_even_when_slug_matches_the_target` proves the
        // selection HELPERS refuse a foreign-venue market, but it REPLICATES the venue filter
        // inside the test, so deleting the production filter would not fail it. This test drives
        // the real `refresh_selection_from_cache` against a shared NT cache holding BOTH a
        // foreign-venue (HYPERLIQUID) and the execution-venue (POLYMARKET) updown market that
        // share the configured target slug, and proves the production path selects ONLY the
        // execution-venue market. Removing the venue-scoped cache filter (the
        // `instrument.id().venue == execution_venue` read) or the
        // `selected_market_on_execution_venue` guard makes this fail. A real order can only ever
        // route to the execution client's venue.
        let mut strategy = test_strategy();
        assert_eq!(
            strategy.context.execution_venue(),
            fixture_execution_venue(),
            "harness precondition: production execution venue must be the POLYMARKET fixture",
        );
        let cache = register_test_strategy(&mut strategy);

        let current_start = 1_746_000_000_i64;
        let now_ms = current_start as u64 * MILLIS_PER_SECOND_U64 + 1;
        let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
            &strategy.config.underlying_asset,
            &strategy.config.cadence_slug_token,
            current_start,
        );
        let start_ms = current_start as u64 * MILLIS_PER_SECOND_U64;
        let end_ms = start_ms + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64;

        // Same slug + market id on a NON-execution venue (HYPERLIQUID) AND the execution venue
        // (POLYMARKET). The matcher is venue-agnostic, so without the production venue filter the
        // foreign instruments would be selectable.
        let mixed = [
            updown_binary_option(
                "token-up.HYPERLIQUID",
                &market_slug,
                "market-1",
                "Up",
                start_ms,
                end_ms,
            ),
            updown_binary_option(
                "token-down.HYPERLIQUID",
                &market_slug,
                "market-1",
                "Down",
                start_ms,
                end_ms,
            ),
            updown_binary_option(
                "token-up.POLYMARKET",
                &market_slug,
                "market-1",
                "Up",
                start_ms,
                end_ms,
            ),
            updown_binary_option(
                "token-down.POLYMARKET",
                &market_slug,
                "market-1",
                "Down",
                start_ms,
                end_ms,
            ),
        ];
        {
            let mut cache_mut = cache.borrow_mut();
            for instrument in &mixed {
                cache_mut
                    .add_instrument(instrument.clone())
                    .expect("test cache should accept the seeded instrument");
            }
        }

        strategy.refresh_selection_from_cache(now_ms);

        // The production venue-scoped read + guard select ONLY the execution-venue market, even
        // though the foreign-venue market carrying the identical slug is present in the cache.
        assert_eq!(
            strategy
                .active
                .books
                .up
                .instrument_id
                .map(|id| id.to_string())
                .as_deref(),
            Some("token-up.POLYMARKET"),
            "production refresh must select the execution-venue Up outcome from a mixed-venue cache",
        );
        assert_eq!(
            strategy
                .active
                .books
                .down
                .instrument_id
                .map(|id| id.to_string())
                .as_deref(),
            Some("token-down.POLYMARKET"),
            "production refresh must select the execution-venue Down outcome from a mixed-venue cache",
        );
    }

    #[test]
    fn bootstrap_recovery_from_cache_ignores_foreign_venue_position() {
        // P5-5 / Codex P5 — RECOVERY-PATH regression lock. The entry path scopes selection to the
        // execution venue; the recovery path must do the same. A foreign-venue cached position with
        // a supported contract shape must NOT be accepted into Managed state, because the exit
        // submission path uses the position's instrument_id directly with no additional venue gate.
        let mut strategy = test_strategy();
        assert_eq!(
            strategy.context.execution_venue(),
            fixture_execution_venue(),
            "harness precondition: production execution venue must be the POLYMARKET fixture",
        );
        let cache = register_test_strategy(&mut strategy);

        // Foreign-venue (HYPERLIQUID) instrument and position
        let foreign_instrument = updown_binary_option(
            "token-up.HYPERLIQUID",
            "foreign-market",
            "market-foreign",
            "Up",
            1_000,
            2_000,
        );
        let foreign_fill = order_filled_event_with_details(
            ClientOrderId::from("FOREIGN-ORDER-001"),
            foreign_instrument.id(),
            Some(PositionId::from("POS-FOREIGN-001")),
            OrderSide::Buy,
        );
        let foreign_position = Position::new(&foreign_instrument, foreign_fill);

        // Seed the cache with the foreign-venue instrument and position
        {
            let mut cache_mut = cache.borrow_mut();
            cache_mut
                .add_instrument(foreign_instrument.clone())
                .expect("test cache should accept the seeded instrument");
            cache_mut
                .add_position(&foreign_position, NtOmsType::Netting)
                .expect("test cache should accept the seeded position");
        }

        // Verify the position is present when querying WITHOUT venue scoping
        assert_eq!(
            cache
                .borrow()
                .positions_open(
                    None,
                    None,
                    Some(&StrategyId::from("BINARYORACLEEDGETAKER-001")),
                    None,
                    None,
                )
                .len(),
            1,
            "foreign position must exist in the unscoped cache",
        );

        strategy.bootstrap_recovery_from_cache();

        // The foreign-venue position must be ignored; strategy stays Flat.
        assert!(
            matches!(strategy.exposure, ExposureState::Flat),
            "a foreign-venue cached position must NOT be recovered into Managed state: got {:?}",
            strategy.exposure,
        );
    }

    #[test]
    fn bootstrap_recovery_from_cache_loads_execution_venue_position() {
        // Baseline: an execution-venue position matching the strategy contract IS recovered into
        // Managed state. This ensures the venue filter does not over-reject.
        let mut strategy = test_strategy();
        assert_eq!(
            strategy.context.execution_venue(),
            fixture_execution_venue(),
            "harness precondition: production execution venue must be the POLYMARKET fixture",
        );
        let cache = register_test_strategy(&mut strategy);

        let execution_instrument = updown_binary_option(
            "token-up.POLYMARKET",
            "execution-market",
            "market-execution",
            "Up",
            1_000,
            2_000,
        );
        let execution_fill = order_filled_event_with_details(
            ClientOrderId::from("EXEC-ORDER-001"),
            execution_instrument.id(),
            Some(PositionId::from("POS-EXEC-001")),
            OrderSide::Buy,
        );
        let execution_position = Position::new(&execution_instrument, execution_fill);

        {
            let mut cache_mut = cache.borrow_mut();
            cache_mut
                .add_instrument(execution_instrument.clone())
                .expect("test cache should accept the seeded instrument");
            cache_mut
                .add_position(&execution_position, NtOmsType::Netting)
                .expect("test cache should accept the seeded position");
        }

        strategy.bootstrap_recovery_from_cache();

        let managed = strategy
            .managed_position()
            .expect("execution-venue position must be recovered into Managed state");
        assert_eq!(
            managed.position.instrument_id.to_string(),
            "token-up.POLYMARKET",
            "recovered position must be the execution-venue instrument",
        );
        assert_eq!(
            managed.position.position_id.to_string(),
            "POS-EXEC-001",
            "recovered position must carry the correct position id",
        );
        assert!(
            matches!(managed.origin, ManagedPositionOrigin::RecoveryBootstrap),
            "recovered position must carry RecoveryBootstrap origin",
        );
    }

    #[test]
    fn strategy_selects_next_updown_target_outcome_from_nt_binary_option_metadata() {
        let strategy = test_strategy();
        let current_start = 1_746_000_000_i64;
        let next_start = current_start + strategy.config.cadence_seconds as i64;
        let market_slug = crate::bolt_v3_market_families::updown::updown_market_slug(
            &strategy.config.underlying_asset,
            &strategy.config.cadence_slug_token,
            next_start,
        );
        let instruments = vec![
            updown_binary_option(
                "token-up.POLYMARKET",
                &market_slug,
                "market-next",
                "Up",
                next_start as u64 * MILLIS_PER_SECOND_U64,
                next_start as u64 * MILLIS_PER_SECOND_U64
                    + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64,
            ),
            updown_binary_option(
                "token-down.POLYMARKET",
                &market_slug,
                "market-next",
                "Down",
                next_start as u64 * MILLIS_PER_SECOND_U64,
                next_start as u64 * MILLIS_PER_SECOND_U64
                    + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64,
            ),
        ];

        let snapshot = selection_snapshot_from_instruments(
            &strategy.config,
            &instruments,
            current_start as u64 * MILLIS_PER_SECOND_U64 + 1,
        );

        let SelectionState::Active { market } = snapshot.decision.state else {
            panic!("configured target should select next market: {snapshot:?}");
        };
        assert_eq!(market.market_id, "market-next");
        assert_eq!(market.selection_outcome, MarketSelectionOutcome::Next);
        assert_eq!(
            market.expiration_ts_ms,
            next_start as u64 * MILLIS_PER_SECOND_U64
                + strategy.config.cadence_seconds * MILLIS_PER_SECOND_U64
        );
    }

    #[test]
    fn warmup_requires_consecutive_fresh_ticks() {
        let mut strategy = test_strategy();
        strategy.config.warmup_tick_count = 3;
        let mut snapshot = active_snapshot("MKT-1");
        let SelectionState::Active { market } = &mut snapshot.decision.state else {
            panic!("expected active snapshot");
        };
        market.price_to_beat = Some(3_100.0);
        strategy.apply_selection_snapshot(snapshot);

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
                EntryBlockReason::ForcedFlat(ForcedFlatReason::StaleReference),
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
    fn entry_gate_blocks_when_active_outcome_book_is_strictly_crossed() {
        // Normal book: bid < ask is not crossed.
        let normal = ready_to_trade_strategy();
        assert!(
            normal.active.books.up.best_bid.unwrap() < normal.active.books.up.best_ask.unwrap()
        );
        assert!(
            !normal
                .entry_gate_decision_at(2_000)
                .blocked_by
                .contains(&EntryBlockReason::BookCrossed),
            "normal bid<ask book must not trip the crossed-book guard"
        );

        // Up book strictly crossed: best_bid > best_ask blocks entry.
        let mut up_crossed = ready_to_trade_strategy();
        up_crossed.active.books.up.best_bid = Some(0.46);
        up_crossed.active.books.up.best_ask = Some(0.45);
        assert!(
            up_crossed
                .entry_gate_decision_at(2_000)
                .blocked_by
                .contains(&EntryBlockReason::BookCrossed),
            "strictly crossed up book must block entry"
        );

        // Down book strictly crossed is detected too (gate treats both books as active).
        let mut down_crossed = ready_to_trade_strategy();
        down_crossed.active.books.down.best_bid = Some(0.46);
        down_crossed.active.books.down.best_ask = Some(0.45);
        assert!(
            down_crossed
                .entry_gate_decision_at(2_000)
                .blocked_by
                .contains(&EntryBlockReason::BookCrossed),
            "strictly crossed down book must block entry"
        );

        // Locked book (bid == ask) is intentionally not crossed.
        let mut locked = ready_to_trade_strategy();
        locked.active.books.up.best_bid = Some(0.45);
        locked.active.books.up.best_ask = Some(0.45);
        assert!(
            !locked
                .entry_gate_decision_at(2_000)
                .blocked_by
                .contains(&EntryBlockReason::BookCrossed),
            "locked bid==ask book must not trip the crossed-book guard"
        );
    }

    #[test]
    fn reference_spot_spike_sets_cooldown_and_blocks_then_allows_entry() {
        let mut strategy = ready_to_trade_strategy();
        // Threshold from the test config fixture.
        assert_eq!(strategy.config.spike_guard_return_threshold, 0.05);
        assert_eq!(strategy.config.spike_guard_cooldown_secs, 5);

        // Seed a previous reference-spot observation so the next one has a baseline.
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 100.0, 1_000));

        // A jump from 100.0 -> 110.0 is a 10% single-step move, >= the 5% threshold.
        strategy.pricing.observe_reference_quote(
            &fast_spot("bybit", 110.0, 2_000),
            strategy.config.lead_agreement_min_corr,
            strategy.config.lead_jitter_max_ms,
            strategy.config.spike_guard_return_threshold,
            strategy.config.spike_guard_cooldown_secs,
        );

        // Cooldown deadline = observed_ts (2_000ms) + 5s * 1_000ms = 7_000ms.
        assert_eq!(strategy.pricing.spike_until_ms, Some(7_000));

        // Entry is blocked while now_ms < spike_until_ms.
        assert!(
            strategy
                .entry_gate_decision_at(6_999)
                .blocked_by
                .contains(&EntryBlockReason::SpotSpikeCooldown),
            "entry must be blocked before the spike cooldown deadline"
        );
        // Boundary: at the deadline the cooldown has elapsed (now_ms < deadline is false).
        assert!(
            !strategy
                .entry_gate_decision_at(7_000)
                .blocked_by
                .contains(&EntryBlockReason::SpotSpikeCooldown),
            "entry must be allowed once now_ms reaches the spike cooldown deadline"
        );
        assert!(
            !strategy
                .entry_gate_decision_at(7_001)
                .blocked_by
                .contains(&EntryBlockReason::SpotSpikeCooldown),
            "entry must be allowed after the spike cooldown deadline"
        );
    }

    #[test]
    fn sub_threshold_reference_spot_move_does_not_arm_spike_cooldown() {
        let mut strategy = ready_to_trade_strategy();
        strategy.pricing.spike_until_ms = None;
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 100.0, 1_000));

        // A 2% move (100.0 -> 102.0) is below the 5% threshold.
        strategy.pricing.observe_reference_quote(
            &fast_spot("bybit", 102.0, 2_000),
            strategy.config.lead_agreement_min_corr,
            strategy.config.lead_jitter_max_ms,
            strategy.config.spike_guard_return_threshold,
            strategy.config.spike_guard_cooldown_secs,
        );

        assert_eq!(
            strategy.pricing.spike_until_ms, None,
            "sub-threshold move must not arm the spike cooldown"
        );
        assert!(
            !strategy
                .entry_gate_decision_at(2_001)
                .blocked_by
                .contains(&EntryBlockReason::SpotSpikeCooldown)
        );
    }

    #[test]
    fn spike_detection_requires_a_valid_previous_observation() {
        let mut strategy = ready_to_trade_strategy();
        strategy.pricing.fast_spot = None;
        strategy.pricing.spike_until_ms = None;

        // First observation has no baseline; a spike cannot be inferred.
        strategy.pricing.observe_reference_quote(
            &fast_spot("bybit", 110.0, 2_000),
            strategy.config.lead_agreement_min_corr,
            strategy.config.lead_jitter_max_ms,
            strategy.config.spike_guard_return_threshold,
            strategy.config.spike_guard_cooldown_secs,
        );

        assert_eq!(
            strategy.pricing.spike_until_ms, None,
            "no previous observation means no spike"
        );
    }

    #[test]
    fn spike_cooldown_deadline_only_extends_never_retracts() {
        // The spike cooldown is a fail-closed safety gate: an out-of-order spike
        // quote carrying an earlier timestamp must never shorten an active
        // cooldown (which would prematurely re-enable entry during volatility).
        let mut strategy = ready_to_trade_strategy();

        // Pre-arm an active cooldown deadline at 7_000ms with a seeded baseline.
        strategy.pricing.spike_until_ms = Some(7_000);
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 100.0, 1_000));

        // Out-of-order spike: 100 -> 130 (30% >= 5% threshold) at an earlier ts
        // (1_500ms). Its naive deadline 1_500 + 5_000 = 6_500ms is before the
        // active 7_000ms and must not retract it.
        strategy.pricing.observe_reference_quote(
            &fast_spot("bybit", 130.0, 1_500),
            strategy.config.lead_agreement_min_corr,
            strategy.config.lead_jitter_max_ms,
            strategy.config.spike_guard_return_threshold,
            strategy.config.spike_guard_cooldown_secs,
        );
        assert_eq!(
            strategy.pricing.spike_until_ms,
            Some(7_000),
            "an out-of-order spike must not shorten an active cooldown deadline"
        );

        // A later spike further into the future extends the deadline forward.
        // Reset the baseline so detection is independent of eligibility chaining.
        strategy.pricing.fast_spot = Some(fast_spot("bybit", 100.0, 1_000));
        strategy.pricing.observe_reference_quote(
            &fast_spot("bybit", 130.0, 4_000),
            strategy.config.lead_agreement_min_corr,
            strategy.config.lead_jitter_max_ms,
            strategy.config.spike_guard_return_threshold,
            strategy.config.spike_guard_cooldown_secs,
        );
        assert_eq!(
            strategy.pricing.spike_until_ms,
            Some(9_000),
            "a later spike (ts 4_000 + 5s) must extend the deadline to 9_000ms"
        );
    }

    #[test]
    fn taker_hardening_guards_are_entry_only_and_do_not_block_exits() {
        // Exits must always be able to fire (risk-off), even with a crossed book
        // and an armed spike cooldown. The exit path is structurally independent
        // of `entry_gate_decision_at`, so neither new gate reason can reach it.
        let configured_instruments = configured_outcome_instruments(
            &ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO),
        );
        for instrument_id in configured_instruments {
            let mut strategy = ready_to_trade_strategy_with_live_fees(Decimal::ZERO, Decimal::ZERO);
            strategy.config.exit_order.order_type = OrderType::Limit;
            strategy.config.exit_order.time_in_force = TimeInForce::Gtc;
            strategy.config.exit_order.is_post_only = true;
            strategy.active.phase = SelectionPhase::Freeze;
            let position_quantity = Quantity::new(strategy.config.order_notional_target, 2);
            materialize_managed_position_with_resting_pending_entry(
                &mut strategy,
                instrument_id,
                PositionId::from(format!("POSITION-ENTRY-ONLY-{instrument_id}").as_str()),
                position_quantity,
            );

            // Cross both active books and arm the spike cooldown well into the future.
            strategy.active.books.up.best_bid = Some(0.46);
            strategy.active.books.up.best_ask = Some(0.45);
            strategy.active.books.down.best_bid = Some(0.46);
            strategy.active.books.down.best_ask = Some(0.45);
            strategy.pricing.spike_until_ms = Some(1_000_000);

            // The entry gate is blocked by both new guards...
            let gate = strategy.entry_gate_decision_at(1_200);
            assert!(
                gate.blocked_by.contains(&EntryBlockReason::BookCrossed),
                "{instrument_id}: crossed book must block entry"
            );
            assert!(
                gate.blocked_by
                    .contains(&EntryBlockReason::SpotSpikeCooldown),
                "{instrument_id}: armed spike cooldown must block entry"
            );

            // ...but the exit still submits.
            let decision = strategy.exit_submission_decision_at(1_200);
            assert_eq!(decision.blocked_reason, None, "{instrument_id}");
            assert_eq!(
                decision.forced_flat_reasons,
                vec![ForcedFlatReason::Freeze],
                "{instrument_id}"
            );
            assert_eq!(
                decision.order_side,
                Some(OrderSide::Sell),
                "{instrument_id}"
            );
            assert!(decision.instrument_id.is_some(), "{instrument_id}");
            assert!(decision.quantity.is_some(), "{instrument_id}");
        }
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
            EXIT_BLOCK_REASON_NO_OPEN_POSITION
        )));
        assert!(!should_warn_on_exit_submission_block(Some(
            EXIT_BLOCK_REASON_EXIT_ALREADY_PENDING
        )));
        assert!(!should_warn_on_exit_submission_block(Some(
            EXIT_BLOCK_REASON_ENTRY_ORDER_STILL_WORKING
        )));
        assert!(!should_warn_on_exit_submission_block(Some(
            EXIT_BLOCK_REASON_EXIT_HOLD
        )));
        assert!(should_warn_on_exit_submission_block(Some(
            EXIT_BLOCK_REASON_EXIT_PRICE_MISSING
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
    fn task5_missing_reference_timestamp_is_stale_reference() {
        // A14: a never-observed reference (None timestamp) is the maximally
        // stale condition and must classify as StaleReference, not as fresh.
        let reasons = evaluate_forced_flat_predicates(&ForcedFlatInputs {
            phase: SelectionPhase::Active,
            metadata_matches_selection: true,
            last_reference_ts_ms: None,
            now_ms: 1_250,
            stale_reference_after_ms: 1_500,
            liquidity_available: Some(500.0),
            min_liquidity_required: 100.0,
            fast_venue_incoherent: false,
        });

        assert_eq!(reasons, vec![ForcedFlatReason::StaleReference]);
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
    fn strategy_input_evidence_records_source_bound_entry_snapshot_before_order_intent() {
        let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
        let submit_admission =
            submit_admission_armed_with_cap(Decimal::new(1, 2), evidence.clone());
        let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
            evidence.clone(),
            submit_admission,
        );
        register_test_strategy_with_active_instruments(&mut strategy);

        let error = strategy
            .try_submit_entry_order(1_200)
            .expect_err("submit admission should reject after evidence capture");
        assert!(
            error.to_string().contains("notional cap is exceeded"),
            "{error:#}"
        );

        let events = evidence.events();
        let [
            RecordedDecisionEvidenceEvent::StrategyInput(snapshot),
            RecordedDecisionEvidenceEvent::OrderIntent(intent),
            RecordedDecisionEvidenceEvent::AdmissionDecision(admission),
        ] = events.as_slice()
        else {
            panic!("expected strategy input, order intent, admission sequence; got {events:#?}");
        };

        assert_eq!(snapshot.strategy_id, strategy.config.strategy_id);
        assert_eq!(snapshot.gate_session_hash, "gate-session-hash-one");
        assert_eq!(snapshot.selected_market_key, "selected-market-key-one");
        assert_eq!(
            snapshot
                .gate_evidence
                .get("resolution_price")
                .and_then(|identity| identity.normalized_value_sha256.as_deref()),
            Some("normalized-value-sha-one")
        );
        assert_eq!(snapshot.price_to_beat_value, "3100");
        assert_eq!(snapshot.reference_quote_ts_event, 1_200);
        assert_eq!(snapshot.spot_price, "3100.5");
        assert_eq!(snapshot.realized_volatility, "1.5");
        assert_eq!(snapshot.seconds_to_market_end, 300);
        assert_eq!(snapshot.market_id.as_deref(), Some("MKT-1"));
        assert_eq!(
            snapshot.polymarket_condition_id.as_deref(),
            Some("condition-MKT-1")
        );
        assert_eq!(
            snapshot.polymarket_market_slug.as_deref(),
            Some("slug-MKT-1")
        );
        assert_eq!(
            snapshot.polymarket_question_id.as_deref(),
            Some("question-MKT-1")
        );
        assert_eq!(
            snapshot.up_instrument_id.as_deref(),
            Some("condition-MKT-1-MKT-1-UP.POLYMARKET")
        );
        assert_eq!(
            snapshot.down_instrument_id.as_deref(),
            Some("condition-MKT-1-MKT-1-DOWN.POLYMARKET")
        );
        assert_eq!(snapshot.selected_side.as_deref(), Some("up"));
        assert_eq!(snapshot.submission_instrument_id, intent.instrument_id);
        assert_eq!(snapshot.submission_order_side, intent.order_side);
        assert_eq!(snapshot.submission_price, intent.price);
        assert_eq!(snapshot.submission_quantity, intent.quantity);
        assert_eq!(snapshot.client_order_id, intent.client_order_id);
        assert_eq!(admission.client_order_id, intent.client_order_id);
        assert_eq!(
            admission.outcome,
            crate::bolt_v3_decision_evidence::BoltV3AdmissionOutcome::RejectedNotionalCapExceeded
        );
    }

    #[test]
    fn strategy_input_evidence_market_end_uses_selection_expiry_not_remaining_seconds() {
        let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
        let submit_admission =
            submit_admission_armed_with_cap(Decimal::new(1, 2), evidence.clone());
        let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
            evidence.clone(),
            submit_admission,
        );
        register_test_strategy_with_active_instruments(&mut strategy);
        strategy.active.interval_end_ms = Some(301_999);

        strategy
            .try_submit_entry_order(2_000)
            .expect_err("submit admission should reject after evidence capture");

        let events = evidence.events();
        let Some(RecordedDecisionEvidenceEvent::StrategyInput(snapshot)) = events.first() else {
            panic!("expected first evidence event to be strategy input; got {events:#?}");
        };

        assert_eq!(snapshot.seconds_to_market_end, 299);
        assert_eq!(snapshot.market_selection_timestamp_ms, Some(1_000));
        assert_eq!(snapshot.polymarket_market_start_timestamp_ms, Some(1_000));
        assert_eq!(
            snapshot.polymarket_market_end_timestamp_ms,
            Some(301_999),
            "market end must bind to selected expiration without seconds rounding"
        );
    }

    #[test]
    fn strategy_input_evidence_records_next_market_selection_outcome() {
        let evidence = Arc::new(RecordingSequencedDecisionEvidenceWriter::default());
        let submit_admission =
            submit_admission_armed_with_cap(Decimal::new(1, 2), evidence.clone());
        let mut strategy = ready_to_trade_strategy_with_decision_evidence_and_submit_admission(
            evidence.clone(),
            submit_admission,
        );
        register_test_strategy_with_active_instruments(&mut strategy);
        strategy.active.market_selection_outcome = MarketSelectionOutcome::Next;

        strategy
            .try_submit_entry_order(2_000)
            .expect_err("submit admission should reject after evidence capture");

        let events = evidence.events();
        let Some(RecordedDecisionEvidenceEvent::StrategyInput(snapshot)) = events.first() else {
            panic!("expected first evidence event to be strategy input; got {events:#?}");
        };

        assert_eq!(snapshot.market_selection_outcome, "next");
    }

    #[test]
    fn same_market_transition_replaces_changed_selection_metadata() {
        let mut active =
            ActiveMarketState::from_snapshot(&active_snapshot_with_start("MKT-1", 1_000), 0);
        assert_eq!(
            active.market_selection_outcome,
            MarketSelectionOutcome::Current
        );
        assert_eq!(active.interval_end_ms, Some(301_000));

        let mut next_market = candidate_market("MKT-1", 1_000);
        next_market.selection_outcome = MarketSelectionOutcome::Next;
        next_market.expiration_ts_ms = 301_999;
        let next = selection_snapshot(
            1_000,
            SelectionState::Active {
                market: next_market,
            },
        );

        apply_selection_snapshot_to_active(&mut active, &next, 0);

        assert_eq!(
            active.market_selection_outcome,
            MarketSelectionOutcome::Next
        );
        assert_eq!(active.interval_end_ms, Some(301_999));
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
        let client_order_id = ClientOrderId::from("ENTRY-HIST-FEE-001");
        let pending = pending_entry_state(&mut strategy, client_order_id);
        let instrument_id = pending.instrument_id;
        set_pending_entry(&mut strategy, pending);

        fee_provider.set_fee(&instrument_id.to_string(), Decimal::new(300, 2));
        strategy
            .on_order_filled(&order_filled_event(
                client_order_id,
                instrument_id,
                PositionId::from("P-HIST-FEE-001"),
            ))
            .expect("entry fill should materialize position for exit EV test");

        let order_config = strategy
            .normal_exit_order_execution_config()
            .expect("normal exit order config should parse");
        let outcome_side = managed_position_ref(&strategy)
            .and_then(|position| position.outcome_side)
            .expect("entry fill should preserve configured outcome side");
        let exit_ev_bps = strategy
            .current_exit_ev_bps_at(outcome_side, &order_config)
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
        let client_order_id = ClientOrderId::from("ENTRY-HIST-LOG-001");
        let pending = pending_entry_state(&mut strategy, client_order_id);
        let instrument_id = pending.instrument_id;
        set_pending_entry(&mut strategy, pending);

        fee_provider.set_fee(&instrument_id.to_string(), Decimal::new(300, 2));
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
            pending_entry: None,
        };
        let mut exit_pending = ExitPendingState {
            position: Some(managed.clone()),
            pending_exit: PendingExitState {
                client_order_id: ClientOrderId::from("EXIT-STATE-001"),
                market_id: Some("MKT-1".to_string()),
                position_id: Some(PositionId::from("P-EXIT-STATE-001")),
                fill_received: false,
                close_received: false,
                terminal_received: false,
                residual_position_observed_after_fill: false,
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
            pending_entry: None,
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
