//! Live binary-oracle market-maker NautilusTrader strategy shell (bolt-v3 W2–W6
//! integration).
//!
//! This is the integrating NT strategy that wires the pure, individually-tested
//! `maker_*` building-block modules into a running [`DataActor`]/[`Strategy`].
//! The maker quotes both outcome legs (YES/NO) of a binary market around an
//! oracle fair value, gated behind the node-global no-submit admission state
//! ([`crate::bolt_v3_submit_admission::BoltV3SubmitAdmissionState`]). It mirrors
//! the taker ([`crate::strategies::binary_oracle_edge_taker`]) for every NT
//! integration detail: the `core: StrategyCore`-first struct, the two-surface
//! hook split (a hand-written `impl DataActor` block for the `&event ->
//! Result<()>` hooks and a [`nautilus_strategy!`] macro block for the by-value
//! Strategy order/position hooks), and a single admit-gated submit chokepoint.
//!
//! ## No-submit safety (non-negotiable)
//!
//! Every path that could submit a live NT order routes through
//! [`BinaryOracleMaker::submit_quote_through_admission`], which calls
//! `self.context.submit_admission().admit(&request)?` BEFORE the NT
//! `Strategy::submit_order(...)`. The node builds the shared admission state
//! `new_unarmed`, so while unarmed `admit` returns `Err(NotArmed)` and every live
//! order is rejected before reaching NT — the maker quotes/computes but cannot
//! fire. There is no second submit path.
//!
//! ## Per-tick pipeline (see [`BinaryOracleMaker::run_quote_tick`])
//!
//! fair_probability_up_for_family -> micro_price + micro_price_anchor ->
//! gm_binary_quote (reservation band) -> inventory_skew -> governor gate ->
//! FamilyQuoteInputs -> MakerFamily::quote_targets -> desired-vs-resting ->
//! requote_needed -> MarketQuote::on_leg_event -> requote_budget gate ->
//! MarketAction translated to NT calls through the admit-gated chokepoint, each
//! submit/requote bumping the per-leg [`ExpectedIdentity`] generation. `None` at
//! any pricing stage is a fail-closed skip (no quote this tick).

use std::{cell::RefCell, collections::BTreeMap, rc::Rc, str::FromStr};

use anyhow::{Context, Result};
use nautilus_common::{actor::DataActor, component::Component, timer::TimeEvent};
#[cfg(not(test))]
use nautilus_model::enums::BookType;
use nautilus_model::{
    data::{OrderBookDeltas, QuoteTick, TradeTick},
    enums::{BookAction, OrderSide, OrderType, PositionSide, TimeInForce},
    events::{OrderCanceled, OrderFilled},
    identifiers::{ClientId, ClientOrderId, InstrumentId, StrategyId},
    instruments::{Instrument, InstrumentAny},
    orders::{Order, OrderAny},
    types::Price,
};
use nautilus_system::trader::Trader;
use nautilus_trading::{Strategy, StrategyConfig, StrategyCore, nautilus_strategy};
use rust_decimal::Decimal;
use serde::Deserialize;
use toml::Value;

use crate::{
    bolt_v3_decision_evidence::{BoltV3OrderIntentEvidence, BoltV3OrderIntentKind},
    bolt_v3_market_families::{self, FairProbabilityInputs},
    bolt_v3_numeric::{
        MILLIS_PER_SECOND_U64, NANOS_PER_MILLI_U64, TWO_F64, ZERO_F64, is_positive_finite,
    },
    bolt_v3_order_intent::{NtOrderBuildInputs, NtOrderTemplate, build_nt_order},
    bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionRequest, BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy,
        base_quantity_admission_notional,
    },
    bolt_v3_trade_flow::{SignedTradeFlow, SignedTradeFlowConfig},
    bolt_v3_volatility::{RealizedVolConfig, RealizedVolEstimator},
    strategies::{
        maker_config::{MakerParametersBlock, ValidatedMakerConfig},
        maker_event_fence::{
            ClientOrderId as FenceClientOrderId, ExpectedIdentity, FenceReject, OrderIdentity,
            VenueReport, VenueReportKind,
        },
        maker_governor::{GovernorInputs, MakerGovernor, MakerGovernorState},
        maker_inventory::MakerInventory,
        maker_maintenance::{maintenance_governor_state, maintenance_posture},
        maker_microprice::{micro_price, micro_price_anchor},
        maker_model::{gm_binary_quote, inventory_skew},
        maker_quote::{
            BinaryFamily, FamilyQuoteInputs, MakerFamily, QuoteSide, QuoteTargetLeg, QuoteTargets,
        },
        maker_resync::{LegReconcileSnapshot, MarketReconcileSnapshot, cancel_all_on_kill},
        maker_settlement::{SettlementOutcome, TokenLot, settle},
        maker_stale_quote::MarketStaleQuoteAlarm,
        quote_lifecycle::{Leg, LegEvent, LifecycleAction, MarketAction, MarketQuote},
        registry::{BoxedStrategy, StrategyBuildContext, StrategyBuilder, ValidationError},
        requote_budget::RequoteBudget,
    },
};

/// The kind key the strategy registry dispatches the maker's TOML config on.
/// Distinct from the taker's `binary_oracle_edge_taker` (NO HARDCODES — the
/// dispatch key is a named const, mirroring the taker's `KEY`).
pub const KEY: &str = stringify!(binary_oracle_maker);

/// REST-call cost of an in-place modify requote (one Modify call). The
/// requote-budget contract (`requote_budget`) charges 1 REST call for a
/// modify-capable venue.
const REQUOTE_COST_MODIFY: u64 = 1;
/// REST-call cost of a cancel+resubmit requote (one cancel + one submit). The
/// requote-budget contract charges 2 REST calls for a no-modify venue.
const REQUOTE_COST_CANCEL_RESUBMIT: u64 = 2;

/// The per-leg fence generation a fresh leg's first order is submitted on (the
/// monotonic counter starts here). Subsequent requotes advance by
/// [`GENERATION_STEP`].
const INITIAL_GENERATION: u64 = 0;
/// The strictly-positive step the per-leg fence generation advances by on each
/// requote, so a fresh order never inherits the stale order's in-flight reports.
const GENERATION_STEP: u64 = 1;

/// Validation error code: the maker config table failed to deserialize.
const DESERIALIZE_FAILED_CODE: &str = stringify!(deserialize_failed);
/// Validation error code: a `[parameters]` knob is outside its domain.
const PARAMETER_OUT_OF_DOMAIN_CODE: &str = stringify!(parameter_out_of_domain);

// ---------------------------------------------------------------------------
// TOML configuration
// ---------------------------------------------------------------------------

/// The maker's NT-runtime and instrument configuration block.
///
/// Every runtime value the shell consumes comes from here or from the nested
/// [`MakerParametersBlock`] `[parameters]` table (NO HARDCODES). Deserialized
/// with `deny_unknown_fields` so a stale/misspelled knob is a loud parse error,
/// matching the taker's archetype contract.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BinaryOracleMakerConfig {
    /// NT strategy id (the `StrategyCore` identity and timer-name prefix).
    pub strategy_id: String,
    /// NT order-id tag.
    pub order_id_tag: String,
    /// Execution client id orders route to.
    pub client_id: String,

    /// Family key for the fair-value seam (`fair_probability_up_for_family`).
    pub family_key: String,
    /// Binary strike price fed to the family fair-value model.
    pub strike_price: f64,

    /// Reference (oracle) instrument id whose quotes drive spot + realized vol.
    pub reference_instrument_id: String,
    /// Reference venue label keyed into the realized-vol estimator.
    pub reference_venue: String,

    /// YES (leg-a / "up") outcome instrument id.
    pub yes_instrument_id: String,
    /// NO (leg-b / "down") outcome instrument id.
    pub no_instrument_id: String,

    /// Per-leg resting quote size in base/share units (sized through NT
    /// `try_make_qty`).
    pub quote_size: f64,

    /// Whether the venue supports in-place order modify (capability fact). When
    /// false, a requote cancels then resubmits (Polymarket binary).
    pub supports_modify: bool,

    /// Quote-refresh cadence in seconds — the requote-tick timer interval.
    pub requote_cadence_seconds: u64,
    /// Ops/heartbeat cadence in seconds — the stale-quote + maintenance timer.
    pub ops_cadence_seconds: u64,

    /// Requote budget: max REST-call cost per rate window.
    pub requote_max_cost_per_window: u64,
    /// Requote budget: rate-window length in milliseconds.
    pub requote_window_ms: u64,

    /// Maximum resting-quote age in ms before the stale-quote alarm refreshes it.
    pub max_rest_age_ms: u64,
    /// Data-gap staleness threshold (ms): a per-leg book/quote gap beyond this in
    /// the watchdog is treated as a probable reconnect.
    pub data_gap_staleness_ms: u64,

    /// Maintenance window start (ms epoch). `window_duration_ms == 0` disables.
    pub maintenance_window_start_ms: u64,
    /// Maintenance window duration in ms (0 = no maintenance window configured).
    pub maintenance_window_duration_ms: u64,
    /// Pre-flatten lead-up before the maintenance window (ms).
    pub maintenance_pre_flatten_lead_ms: u64,

    /// Requote threshold: minimum desired-vs-resting price move that triggers a
    /// requote (family price units).
    pub requote_threshold: f64,

    /// Pricing kurtosis fed to the family fair-value model.
    pub pricing_kurtosis: f64,

    /// Realized-vol window in seconds.
    pub vol_window_secs: u64,
    /// Realized-vol gap-reset in seconds.
    pub vol_gap_reset_secs: u64,
    /// Realized-vol minimum observations before a value is ready.
    pub vol_min_observations: u64,
    /// Realized-vol bridge-validity horizon in seconds.
    pub vol_bridge_valid_secs: u64,

    /// Signed-trade-flow retention window in seconds.
    pub trade_flow_window_secs: u64,
    /// Signed-trade-flow hard sample cap.
    pub trade_flow_max_samples: u64,

    /// The validated pricing/governance/throttle knobs (nested `[parameters]`).
    pub parameters: MakerParametersBlock,
}

impl BinaryOracleMakerConfig {
    fn realized_vol_config(&self) -> RealizedVolConfig {
        RealizedVolConfig {
            window_secs: self.vol_window_secs,
            gap_reset_secs: self.vol_gap_reset_secs,
            min_observations: self.vol_min_observations,
            bridge_valid_secs: self.vol_bridge_valid_secs,
        }
    }

    fn signed_trade_flow_config(&self) -> SignedTradeFlowConfig {
        SignedTradeFlowConfig {
            window_secs: self.trade_flow_window_secs,
            max_samples: self.trade_flow_max_samples,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-leg L2 order book (mirrors the taker's OutcomeBookState shape)
// ---------------------------------------------------------------------------

/// A single outcome leg's L2 book, maintained from `OrderBookDeltas`. Tracks the
/// touch (best bid/ask and their sizes) so the micro-price nudge can read it.
#[derive(Debug, Clone, PartialEq)]
struct MakerLegBook {
    bid_levels: BTreeMap<Price, f64>,
    ask_levels: BTreeMap<Price, f64>,
    best_bid: Option<f64>,
    best_ask: Option<f64>,
    best_bid_size: Option<f64>,
    best_ask_size: Option<f64>,
}

impl MakerLegBook {
    fn empty() -> Self {
        Self {
            bid_levels: BTreeMap::new(),
            ask_levels: BTreeMap::new(),
            best_bid: None,
            best_ask: None,
            best_bid_size: None,
            best_ask_size: None,
        }
    }

    /// Fold a batch of deltas into the book, then refresh the touch. Mirrors the
    /// taker's `OutcomeBookState::update_from_deltas` level bookkeeping.
    fn update_from_deltas(&mut self, deltas: &OrderBookDeltas) {
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
        let best_bid = self.bid_levels.last_key_value();
        let best_ask = self.ask_levels.first_key_value();
        self.best_bid = best_bid.map(|(price, _)| price.as_f64());
        self.best_bid_size = best_bid.map(|(_, size)| *size);
        self.best_ask = best_ask.map(|(price, _)| price.as_f64());
        self.best_ask_size = best_ask.map(|(_, size)| *size);
    }

    /// The order-book mid `(best_bid + best_ask)/2`, when both sides are priced.
    fn venue_mid(&self) -> Option<f64> {
        match (self.best_bid, self.best_ask) {
            (Some(bid), Some(ask)) => Some((bid + ask) / TWO_F64),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-leg lifecycle slot (lifecycle state lives in MarketQuote; the slot holds
// the resting price, fence identity, and last-rest timestamp the shell owns)
// ---------------------------------------------------------------------------

/// The shell-owned per-leg state alongside the pure `MarketQuote` leg: the order
/// fence identity, the desired/resting price, the live NT client order id, the
/// next fence generation to issue, and the timestamp the quote last began
/// resting (for the stale-quote alarm).
#[derive(Debug, Clone)]
struct LegSlot {
    fence: ExpectedIdentity,
    next_generation: u64,
    resting_price: Option<f64>,
    pending_price: Option<f64>,
    client_order_id: Option<ClientOrderId>,
    last_rest_ms: Option<u64>,
}

impl LegSlot {
    fn idle() -> Self {
        Self {
            fence: ExpectedIdentity::idle(),
            next_generation: INITIAL_GENERATION,
            resting_price: None,
            pending_price: None,
            client_order_id: None,
            last_rest_ms: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Maker-side spot / realized-vol feed (NOT the taker's PricingState)
// ---------------------------------------------------------------------------

/// The maker's own spot + realized-vol feed, driven from the reference
/// instrument quote. Deliberately a fresh, minimal feed (it reuses the shared
/// [`RealizedVolEstimator`] from `bolt_v3_volatility`) rather than hoisting the
/// taker's `PricingState`.
#[derive(Debug, Clone)]
struct MakerSpotFeed {
    reference_venue: String,
    last_spot: Option<f64>,
    realized_vol: RealizedVolEstimator,
}

impl MakerSpotFeed {
    fn from_config(config: &BinaryOracleMakerConfig) -> Self {
        Self {
            reference_venue: config.reference_venue.clone(),
            last_spot: None,
            realized_vol: RealizedVolEstimator::from_config(&config.realized_vol_config()),
        }
    }

    /// Observe a reference mid at `observed_ts_ms`, updating spot and the RV
    /// estimator. Non-positive/non-finite mids are ignored (fail-closed).
    fn observe(&mut self, mid: f64, observed_ts_ms: u64) {
        if !is_positive_finite(mid) {
            return;
        }
        self.last_spot = Some(mid);
        let _ = self
            .realized_vol
            .observe(&self.reference_venue, mid, observed_ts_ms);
    }

    fn spot(&self) -> Option<f64> {
        self.last_spot
    }

    fn realized_vol_at(&self, now_ms: u64) -> Option<f64> {
        self.realized_vol.current_vol_at(now_ms)
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// The fixed safety precedence of the governor postures, least-to-most
/// restrictive. A kill is never downgraded back to quoting, so
/// [`most_restrictive`] selects whichever posture sits later in this order. The
/// list is exhaustive over [`MakerGovernorState`] (an added variant breaks the
/// `position_in_precedence` match loudly rather than silently mis-ranking).
const GOVERNOR_PRECEDENCE: &[MakerGovernorState] = &[
    MakerGovernorState::Quoting,
    MakerGovernorState::SoftHold,
    MakerGovernorState::ReduceOnly,
    MakerGovernorState::CancelOnly,
    // `HardFlat` carries a reason; rank it by variant, reason-agnostic.
    MakerGovernorState::HardFlat(crate::strategies::maker_governor::KillReason::TauFloor),
];

/// The more restrictive (safety-wins) of two governor postures: the one later in
/// [`GOVERNOR_PRECEDENCE`]. A kill is never downgraded back to quoting.
fn most_restrictive(a: MakerGovernorState, b: MakerGovernorState) -> MakerGovernorState {
    if position_in_precedence(a) >= position_in_precedence(b) {
        a
    } else {
        b
    }
}

/// The index of a posture within [`GOVERNOR_PRECEDENCE`], matching `HardFlat`
/// reason-agnostically. No bare rank literal: the ordering is the array itself.
fn position_in_precedence(state: MakerGovernorState) -> usize {
    GOVERNOR_PRECEDENCE
        .iter()
        .position(|candidate| {
            matches!(
                (candidate, state),
                (MakerGovernorState::Quoting, MakerGovernorState::Quoting)
                    | (MakerGovernorState::SoftHold, MakerGovernorState::SoftHold)
                    | (
                        MakerGovernorState::ReduceOnly,
                        MakerGovernorState::ReduceOnly
                    )
                    | (
                        MakerGovernorState::CancelOnly,
                        MakerGovernorState::CancelOnly
                    )
                    | (
                        MakerGovernorState::HardFlat(_),
                        MakerGovernorState::HardFlat(_)
                    )
            )
        })
        .unwrap_or(GOVERNOR_PRECEDENCE.len())
}

/// Whether the time since `last_ms` exceeds `threshold_ms` as of `now_ms`. A
/// never-seen leg (`None`) does not count as a gap until its first data arrives.
fn data_gap_exceeds(last_ms: Option<u64>, now_ms: u64, threshold_ms: u64) -> bool {
    match last_ms {
        None => false,
        Some(last_ms) => now_ms.saturating_sub(last_ms) > threshold_ms,
    }
}

// ---------------------------------------------------------------------------
// Strategy struct
// ---------------------------------------------------------------------------

/// The binary-oracle market-maker NT strategy.
pub struct BinaryOracleMaker {
    core: StrategyCore,
    config: BinaryOracleMakerConfig,
    validated: ValidatedMakerConfig,
    context: StrategyBuildContext,

    yes_instrument_id: InstrumentId,
    no_instrument_id: InstrumentId,
    reference_instrument_id_parsed: Option<InstrumentId>,

    market: MarketQuote,
    governor: MakerGovernor,
    inventory: MakerInventory,
    requote_budget: RequoteBudget,
    family: BinaryFamily,

    yes_book: MakerLegBook,
    no_book: MakerLegBook,
    yes_slot: LegSlot,
    no_slot: LegSlot,

    spot: MakerSpotFeed,
    trade_flow: BTreeMap<InstrumentId, SignedTradeFlow>,

    yes_last_data_ms: Option<u64>,
    no_last_data_ms: Option<u64>,

    /// Last settlement booking PnL, retained for observability/tests.
    last_settlement_pnl: Option<f64>,
    /// Test-only capture of the order intents the shell would submit live, used
    /// to assert the MarketAction -> intent translation without a runtime.
    #[cfg(test)]
    submit_attempts: Vec<MakerSubmitAttempt>,
}

/// A captured submit attempt (test instrumentation): the leg, side, price, and
/// whether the admit gate permitted it.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
struct MakerSubmitAttempt {
    leg: Leg,
    side: QuoteSide,
    price: f64,
    admitted: bool,
}

impl std::fmt::Debug for BinaryOracleMaker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinaryOracleMaker")
            .field("config", &self.config)
            .finish()
    }
}

impl BinaryOracleMaker {
    fn new(config: BinaryOracleMakerConfig, context: StrategyBuildContext) -> Self {
        let context_label = format!("strategies[{}].parameters", config.strategy_id);
        // The builder's validate_config already proved this passes; re-running it
        // here is the single validated-knob source and panics loudly if a build
        // somehow bypassed validation (it cannot on the registry path).
        let validated = config
            .parameters
            .validate(&context_label)
            .expect("validated binary_oracle_maker parameters");
        let governor = MakerGovernor::new(validated.kill_thresholds());
        let requote_budget = RequoteBudget::new(
            config.requote_max_cost_per_window,
            config.requote_window_ms,
            validated.requote_min_interval_ms(),
        );
        let spot = MakerSpotFeed::from_config(&config);
        let yes_instrument_id = InstrumentId::from(config.yes_instrument_id.as_str());
        let no_instrument_id = InstrumentId::from(config.no_instrument_id.as_str());
        let reference_instrument_id_parsed =
            InstrumentId::from_str(config.reference_instrument_id.as_str()).ok();
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(StrategyId::from(config.strategy_id.as_str())),
                order_id_tag: Some(config.order_id_tag.clone()),
                ..Default::default()
            }),
            market: MarketQuote::new(config.supports_modify),
            governor,
            inventory: MakerInventory::flat(),
            requote_budget,
            family: BinaryFamily,
            yes_book: MakerLegBook::empty(),
            no_book: MakerLegBook::empty(),
            yes_slot: LegSlot::idle(),
            no_slot: LegSlot::idle(),
            spot,
            trade_flow: BTreeMap::new(),
            yes_last_data_ms: None,
            no_last_data_ms: None,
            last_settlement_pnl: None,
            yes_instrument_id,
            no_instrument_id,
            reference_instrument_id_parsed,
            validated,
            config,
            context,
            #[cfg(test)]
            submit_attempts: Vec::new(),
        }
    }

    // -- identity / timing helpers ------------------------------------------

    fn requote_timer_name(&self) -> String {
        format!("{}:requote", self.config.strategy_id)
    }

    fn ops_timer_name(&self) -> String {
        format!("{}:ops", self.config.strategy_id)
    }

    fn client_id(&self) -> ClientId {
        ClientId::from(self.config.client_id.as_str())
    }

    fn leg_instrument_id(&self, leg: Leg) -> InstrumentId {
        match leg {
            Leg::Yes => self.yes_instrument_id,
            Leg::No => self.no_instrument_id,
        }
    }

    fn leg_book(&self, leg: Leg) -> &MakerLegBook {
        match leg {
            Leg::Yes => &self.yes_book,
            Leg::No => &self.no_book,
        }
    }

    fn leg_slot(&self, leg: Leg) -> &LegSlot {
        match leg {
            Leg::Yes => &self.yes_slot,
            Leg::No => &self.no_slot,
        }
    }

    fn leg_slot_mut(&mut self, leg: Leg) -> &mut LegSlot {
        match leg {
            Leg::Yes => &mut self.yes_slot,
            Leg::No => &mut self.no_slot,
        }
    }

    fn leg_for_instrument(&self, instrument_id: InstrumentId) -> Option<Leg> {
        if instrument_id == self.yes_instrument_id {
            Some(Leg::Yes)
        } else if instrument_id == self.no_instrument_id {
            Some(Leg::No)
        } else {
            None
        }
    }

    fn leg_for_client_order_id(&self, client_order_id: &ClientOrderId) -> Option<Leg> {
        [Leg::Yes, Leg::No].into_iter().find(|&leg| {
            self.leg_slot(leg)
                .client_order_id
                .as_ref()
                .is_some_and(|id| id == client_order_id)
        })
    }

    // -- lifecycle subscriptions --------------------------------------------

    fn subscribe_reference_quotes(&mut self) {
        if let Some(instrument_id) = self.reference_instrument_id_parsed {
            #[cfg(not(test))]
            self.subscribe_quotes(instrument_id, None, None);
            #[cfg(test)]
            let _ = instrument_id;
        }
    }

    fn unsubscribe_reference_quotes(&mut self) {
        if let Some(instrument_id) = self.reference_instrument_id_parsed {
            #[cfg(not(test))]
            self.unsubscribe_quotes(instrument_id, None, None);
            #[cfg(test)]
            let _ = instrument_id;
        }
    }

    /// Subscribe both outcome legs' L2_MBP books + trades, and seed the per-leg
    /// signed-trade-flow buffers. The maker quotes both legs from the start, so
    /// both books are armed eagerly in `on_start` (the taker defers its book
    /// subscriptions until a position exists).
    fn subscribe_both_legs(&mut self) {
        let flow_config = self.config.signed_trade_flow_config();
        for instrument_id in [self.yes_instrument_id, self.no_instrument_id] {
            #[cfg(not(test))]
            {
                self.subscribe_book_deltas(
                    instrument_id,
                    BookType::L2_MBP,
                    None,
                    None,
                    false,
                    None,
                );
                self.subscribe_trades(instrument_id, None, None);
            }
            self.trade_flow
                .insert(instrument_id, SignedTradeFlow::from_config(&flow_config));
        }
    }

    fn unsubscribe_both_legs(&mut self) {
        for instrument_id in [self.yes_instrument_id, self.no_instrument_id] {
            #[cfg(not(test))]
            {
                self.unsubscribe_book_deltas(instrument_id, None, None);
                self.unsubscribe_trades(instrument_id, None, None);
            }
            self.trade_flow.remove(&instrument_id);
        }
    }

    fn register_timers(&mut self) {
        // Capture the strategy id before borrowing the clock: `clock()` returns a
        // `RefMut` whose temporary would otherwise outlive the immutable read of
        // `self.config.strategy_id` in the error-log arm.
        let strategy_id = self.config.strategy_id.clone();
        let requote_name = self.requote_timer_name();
        let requote_interval_ns = self
            .config
            .requote_cadence_seconds
            .saturating_mul(MILLIS_PER_SECOND_U64)
            .saturating_mul(NANOS_PER_MILLI_U64);
        if let Err(error) = self.clock().set_timer_ns(
            &requote_name,
            requote_interval_ns,
            None,
            None,
            None,
            None,
            None,
        ) {
            log::error!(
                "binary_oracle_maker requote timer registration failed: strategy_id={strategy_id} error={error:#}",
            );
        }
        let ops_name = self.ops_timer_name();
        let ops_interval_ns = self
            .config
            .ops_cadence_seconds
            .saturating_mul(MILLIS_PER_SECOND_U64)
            .saturating_mul(NANOS_PER_MILLI_U64);
        if let Err(error) =
            self.clock()
                .set_timer_ns(&ops_name, ops_interval_ns, None, None, None, None, None)
        {
            log::error!(
                "binary_oracle_maker ops timer registration failed: strategy_id={strategy_id} error={error:#}",
            );
        }
    }

    fn deregister_timers(&mut self) {
        let requote_name = self.requote_timer_name();
        let ops_name = self.ops_timer_name();
        self.clock().cancel_timer(requote_name.as_str());
        self.clock().cancel_timer(ops_name.as_str());
    }

    // -- the per-tick quote pipeline ----------------------------------------

    /// Compute the book-nudged fair anchor for the YES outcome (P(up)) from the
    /// oracle + venue top-of-book. `None` at any stage = no quote this tick.
    fn anchored_fair(&self, now_ms: u64) -> Option<f64> {
        let spot = self.spot.spot()?;
        let realized_vol = self.spot.realized_vol_at(now_ms)?;
        let seconds_to_market_end = self.seconds_to_market_end(now_ms);
        let inputs = FairProbabilityInputs {
            spot_price: spot,
            strike_price: self.config.strike_price,
            seconds_to_market_end,
            realized_vol,
            pricing_kurtosis: self.config.pricing_kurtosis,
        };
        let p_up = bolt_v3_market_families::fair_probability_up_for_family(
            &self.config.family_key,
            &inputs,
        )?;
        // Book nudge: read the YES leg's touch and blend toward the oracle prior.
        let yes_book = self.leg_book(Leg::Yes);
        let micro = match (
            yes_book.best_bid,
            yes_book.best_ask,
            yes_book.best_bid_size,
            yes_book.best_ask_size,
        ) {
            (Some(bid), Some(ask), Some(bid_size), Some(ask_size)) => {
                micro_price(bid, ask, bid_size, ask_size)
            }
            _ => None,
        };
        micro_price_anchor(p_up, micro, self.validated.micro_weight())
    }

    /// Seconds remaining to the binary's resolution, derived from the maintenance
    /// horizon when configured. Falls back to the reference-tau horizon so the
    /// family model and governor always have a positive `τ` to read; the τ-floor
    /// kill predicate still fires on a too-small value.
    fn seconds_to_market_end(&self, now_ms: u64) -> u64 {
        if self.config.maintenance_window_duration_ms > 0
            && self.config.maintenance_window_start_ms > now_ms
        {
            (self.config.maintenance_window_start_ms - now_ms) / MILLIS_PER_SECOND_U64
        } else {
            self.validated.reference_tau() as u64
        }
    }

    /// The governor posture for this tick, folding the maintenance gate alongside
    /// the W3 market/inventory governor (safety wins — the more restrictive of the
    /// two postures is taken; a kill is never downgraded back to quoting).
    fn governor_state(&self, now_ms: u64, oracle_fair: f64) -> MakerGovernorState {
        let sigma = self.spot.realized_vol_at(now_ms).unwrap_or(f64::NAN);
        let venue_mid = self.leg_book(Leg::Yes).venue_mid().unwrap_or(f64::NAN);
        let tau = self.seconds_to_market_end(now_ms) as f64;
        let market_state = self.governor.resolve(GovernorInputs {
            sigma,
            oracle_fair,
            venue_mid,
            tau,
            net_position: self.inventory.net_position(),
        });
        // Fold the maintenance gate ONLY when a window is configured. A duration
        // of zero means "no maintenance window configured" (not a degenerate
        // window), so the gate must not veto — consulting it would fail-closed to
        // CancelOnly and silently stop all quoting. A configured window with a
        // genuinely degenerate shape still fails closed inside the gate.
        match self.maintenance_governor_state(now_ms) {
            Some(maintenance_state) => most_restrictive(market_state, maintenance_state),
            None => market_state,
        }
    }

    /// The maintenance-gate posture for this tick, or `None` when no maintenance
    /// window is configured (`maintenance_window_duration_ms == 0`). A configured
    /// window resolves through `maintenance_posture` -> `maintenance_governor_state`
    /// (which itself fails closed to CancelOnly on a degenerate configured shape).
    fn maintenance_governor_state(&self, now_ms: u64) -> Option<MakerGovernorState> {
        if self.config.maintenance_window_duration_ms == 0 {
            return None;
        }
        Some(maintenance_governor_state(maintenance_posture(
            now_ms,
            self.config.maintenance_window_start_ms,
            self.config.maintenance_window_duration_ms,
            self.config.maintenance_pre_flatten_lead_ms,
        )))
    }

    /// Build the two desired quote leg prices for this tick from the full maker
    /// pipeline. `None` = no quotable target (fail-closed).
    fn desired_quote_targets(&self, oracle_fair: f64) -> Option<QuoteTargets> {
        let gm = gm_binary_quote(oracle_fair, self.validated.informed_fraction())?;
        let skew = inventory_skew(
            self.inventory.net_position(),
            self.validated.skew_gain(),
            self.validated.position_cap(),
        )?;
        let inputs = FamilyQuoteInputs {
            fair: oracle_fair,
            reservation_bid: gm.bid,
            reservation_ask: gm.ask,
            inventory_skew: skew,
            half_spread_floor: self.validated.half_spread_floor(),
            max_half_spread: self.validated.max_half_spread(),
            eps: self.validated.eps(),
            tau: self.validated.reference_tau(),
            reference_tau: self.validated.reference_tau(),
            time_widen_cap: self.validated.time_widen_cap(),
        };
        self.family.quote_targets(inputs)
    }

    /// Whether `desired` has moved beyond the requote threshold from `resting`.
    /// A leg with no resting price always needs a (first) quote.
    fn requote_needed(&self, resting: Option<f64>, desired: f64) -> bool {
        match resting {
            None => true,
            Some(resting) => (desired - resting).abs() >= self.config.requote_threshold,
        }
    }

    /// The cost of a requote in REST-call units, from the venue's modify
    /// capability (1 for modify-in-place, 2 for cancel+resubmit).
    fn requote_cost(&self) -> u64 {
        if self.config.supports_modify {
            REQUOTE_COST_MODIFY
        } else {
            REQUOTE_COST_CANCEL_RESUBMIT
        }
    }

    fn target_leg(targets: &QuoteTargets, leg: Leg) -> QuoteTargetLeg {
        match leg {
            Leg::Yes => targets.leg_a,
            Leg::No => targets.leg_b,
        }
    }

    /// The main quote loop — runs on the requote timer and on each book delta.
    fn run_quote_tick(&mut self, now_ms: u64) {
        let Some(oracle_fair) = self.anchored_fair(now_ms) else {
            return;
        };
        let posture = self.governor_state(now_ms, oracle_fair);

        // Safety postures: drain / reduce-one-side BEFORE any quoting.
        match posture {
            MakerGovernorState::HardFlat(_) | MakerGovernorState::CancelOnly => {
                if let Some(action) = cancel_all_on_kill(posture, &mut self.market) {
                    self.execute_action(action, None, now_ms);
                }
                return;
            }
            MakerGovernorState::ReduceOnly => {
                // Cancel the inventory-adding side; keep the reducing side. Net
                // long YES => stop adding YES (cancel YES side); net short =>
                // cancel NO side. A flat-but-capped book cancels neither.
                let net = self.inventory.net_position();
                if net > ZERO_F64
                    && let Some(action) = self.market.cancel_one_side(Leg::Yes)
                {
                    self.execute_action(action, None, now_ms);
                } else if net < ZERO_F64
                    && let Some(action) = self.market.cancel_one_side(Leg::No)
                {
                    self.execute_action(action, None, now_ms);
                }
                return;
            }
            MakerGovernorState::SoftHold => {
                // W7 reward-preserving hold: add no new directional risk. W3 never
                // produces it; treat as quoting suppression for safety.
                return;
            }
            MakerGovernorState::Quoting => {}
        }

        let Some(targets) = self.desired_quote_targets(oracle_fair) else {
            return;
        };

        for leg in [Leg::Yes, Leg::No] {
            let target = Self::target_leg(&targets, leg);
            let resting = self.leg_slot(leg).resting_price;
            let requote_needed = self.requote_needed(resting, target.price);
            let Some(action) = self
                .market
                .on_leg_event(leg, LegEvent::QuoteTrigger { requote_needed })
            else {
                continue;
            };
            // Throttle gate: only act on the lifecycle action if the budget allows.
            if !self.requote_budget.try_acquire(now_ms, self.requote_cost()) {
                continue;
            }
            self.leg_slot_mut(leg).pending_price = Some(target.price);
            self.execute_action(action, Some(target), now_ms);
        }
    }

    // -- MarketAction -> NT translation -------------------------------------

    /// Translate one [`MarketAction`] from the pure lifecycle/governor layer into
    /// NT order calls. `target` carries the desired leg price when a Submit/Modify
    /// is in play. All live submits route through the admit gate.
    fn execute_action(
        &mut self,
        action: MarketAction,
        target: Option<QuoteTargetLeg>,
        now_ms: u64,
    ) {
        match action {
            MarketAction::Leg { leg, action } => match action {
                LifecycleAction::Submit | LifecycleAction::Modify => {
                    if let Some(target) = target {
                        self.submit_or_modify_leg(leg, target, now_ms);
                    }
                }
                LifecycleAction::Cancel => self.cancel_leg(leg),
            },
            MarketAction::CancelAllBothLegs => {
                self.cancel_all(None);
                self.yes_slot = LegSlot::idle();
                self.no_slot = LegSlot::idle();
            }
            MarketAction::CancelAllOneSide { leg } => {
                // Both binary legs rest BIDS; the cancel scope by instrument side
                // for a resting bid is Buy.
                self.cancel_all_one_side(leg, OrderSide::Buy);
                *self.leg_slot_mut(leg) = LegSlot::idle();
            }
        }
    }

    /// Submit (or, on a modify-capable venue, replace via the same submit path) a
    /// post-only limit bid for one leg at `target.price`, bumping the fence
    /// generation, THROUGH the admit gate. A binary maker rests bids on both
    /// outcome tokens, so `QuoteSide::Buy` maps to `OrderSide::Buy`.
    /// Generate the next client order id for a leg's submit. In production this is
    /// NT's `OrderFactory` (the single id authority). Under `#[cfg(test)]` — where
    /// no runtime is registered and `order_factory()` would panic — it returns a
    /// deterministic synthetic id so the pure quote-loop translation is testable
    /// without a runtime (NT order calls are themselves `#[cfg(not(test))]`).
    fn next_client_order_id(&mut self, leg: Leg) -> ClientOrderId {
        #[cfg(not(test))]
        {
            let _ = leg;
            self.core.order_factory().generate_client_order_id()
        }
        #[cfg(test)]
        {
            let generation = self.leg_slot(leg).next_generation;
            ClientOrderId::from(
                format!("{}-{:?}-{generation}", self.config.strategy_id, leg).as_str(),
            )
        }
    }

    fn submit_or_modify_leg(&mut self, leg: Leg, target: QuoteTargetLeg, now_ms: u64) {
        let instrument_id = self.leg_instrument_id(leg);
        let order_side = match target.side {
            QuoteSide::Buy => OrderSide::Buy,
            QuoteSide::Sell => OrderSide::Sell,
        };
        let client_order_id = self.next_client_order_id(leg);
        // Bump the per-leg fence generation BEFORE submit so a fresh order never
        // inherits the stale order's in-flight reports.
        {
            let slot = self.leg_slot_mut(leg);
            let generation = slot.next_generation;
            slot.next_generation = generation.saturating_add(GENERATION_STEP);
            let identity = OrderIdentity::new(
                FenceClientOrderId::new(client_order_id.to_string()),
                generation,
            );
            if generation == 0 {
                slot.fence = ExpectedIdentity::submitting(identity);
            } else if !slot.fence.requote_to(identity) {
                // Generation did not strictly advance — a caller bug; refuse the
                // submit (fail-closed) rather than risk a stale-report match.
                return;
            }
            slot.client_order_id = Some(client_order_id);
            slot.pending_price = Some(target.price);
        }

        match self.submit_quote_through_admission(
            instrument_id,
            order_side,
            target.price,
            client_order_id,
        ) {
            Ok(admitted) => {
                #[cfg(test)]
                self.submit_attempts.push(MakerSubmitAttempt {
                    leg,
                    side: target.side,
                    price: target.price,
                    admitted,
                });
                #[cfg(not(test))]
                let _ = admitted;
                self.leg_slot_mut(leg).last_rest_ms = Some(now_ms);
            }
            Err(error) => {
                #[cfg(test)]
                self.submit_attempts.push(MakerSubmitAttempt {
                    leg,
                    side: target.side,
                    price: target.price,
                    admitted: false,
                });
                log::warn!(
                    "binary_oracle_maker quote submit rejected (gate or build): strategy_id={} leg={:?} instrument_id={} error={:#}",
                    self.config.strategy_id,
                    leg,
                    instrument_id,
                    error,
                );
            }
        }
    }

    /// THE single admit-gated submit chokepoint. Builds the post-only limit
    /// order, builds the admission request, records order-intent evidence, calls
    /// `admit(&request)?`, and ONLY on the returned permit submits to NT. No other
    /// path in this shell calls NT `submit_order`. Returns `Ok(true)` when the
    /// order was admitted and submitted; the gate's `Err(NotArmed)` while unarmed
    /// surfaces as `Err` here, so no live order can escape.
    fn submit_quote_through_admission(
        &mut self,
        instrument_id: InstrumentId,
        order_side: OrderSide,
        price: f64,
        client_order_id: ClientOrderId,
    ) -> Result<bool> {
        let instrument = self
            .current_instrument(instrument_id)
            .with_context(|| format!("missing instrument context for {instrument_id}"))?;
        let order = self.build_quote_order(&instrument, order_side, price, client_order_id)?;
        let request = self.admission_request_from_order(&order)?;
        let intent = BoltV3OrderIntentEvidence::from_compiled_order(
            self.config.strategy_id.clone(),
            BoltV3OrderIntentKind::Entry,
            order
                .price()
                .map(|p| p.to_string())
                .unwrap_or_else(|| price.to_string()),
            &order,
        );
        self.context
            .decision_evidence()
            .record_order_intent(&intent)?;
        // The gate: every live submit passes through admit() first. While the
        // node's shared admission state is unarmed this returns Err(NotArmed),
        // rejecting the order before NT (no-submit safety).
        let _permit = self
            .context
            .submit_admission()
            .admit(&request)
            .context("submit admission rejected")?;
        self.submit_order(order, None, Some(self.client_id()), None)?;
        Ok(true)
    }

    fn build_quote_order(
        &mut self,
        instrument: &InstrumentAny,
        order_side: OrderSide,
        price: f64,
        client_order_id: ClientOrderId,
    ) -> Result<OrderAny> {
        let quantity = instrument
            .try_make_qty(self.config.quote_size, Some(true))
            .context("quote size does not fit the instrument size precision")?;
        let nt_price = Price::new(price, instrument.price_precision());
        // Post-only GTC limit: a maker quote earns rebate / never crosses. No
        // trigger/trailing fields, not reduce-only, not quote-quantity.
        let template = NtOrderTemplate {
            order_type: OrderType::Limit,
            time_in_force: TimeInForce::Gtc,
            expire_time: None,
            trigger_price: None,
            activation_price: None,
            trigger_type: None,
            trigger_instrument_id: None,
            trailing_offset: None,
            trailing_offset_type: None,
            is_post_only: true,
            is_reduce_only: false,
            is_quote_quantity: false,
        };
        build_nt_order(
            self.core.order_factory(),
            KEY,
            &template,
            NtOrderBuildInputs {
                instrument_id: instrument.id(),
                order_side,
                quantity,
                price: Some(nt_price),
                client_order_id,
            },
        )
    }

    fn admission_request_from_order(
        &self,
        order: &OrderAny,
    ) -> Result<BoltV3SubmitAdmissionRequest> {
        let client_order_id = order.client_order_id().to_string();
        let quantity =
            Decimal::from_str(order.quantity().to_string().trim()).with_context(|| {
                format!(
                    "submit admission quantity is not a decimal for client_order_id={client_order_id}"
                )
            })?;
        let price = order
            .price()
            .ok_or_else(|| anyhow::anyhow!("maker quote order missing limit price"))?;
        let price_decimal = Decimal::from_str(price.to_string().trim()).with_context(|| {
            format!("submit admission price is not a decimal for client_order_id={client_order_id}")
        })?;
        let notional = base_quantity_admission_notional(price_decimal, quantity);
        Ok(BoltV3SubmitAdmissionRequest {
            strategy_id: self.config.strategy_id.clone(),
            client_order_id,
            instrument_id: order.instrument_id().to_string(),
            notional,
            intent_kind: BoltV3SubmitIntentKind::Entry,
            lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
            canary_proof_claim: None,
        })
    }

    fn current_instrument(&self, instrument_id: InstrumentId) -> Option<InstrumentAny> {
        if self.is_registered() {
            self.cache().instrument(&instrument_id).cloned()
        } else {
            None
        }
    }

    /// Cancel the single resting order on one leg (per-order cancel via the
    /// leg's tracked client order id).
    fn cancel_leg(&mut self, leg: Leg) {
        let Some(client_order_id) = self.leg_slot(leg).client_order_id else {
            return;
        };
        #[cfg(not(test))]
        {
            if let Err(error) = self.cancel_order(client_order_id, Some(self.client_id()), None) {
                log::error!(
                    "binary_oracle_maker cancel_leg failed: strategy_id={} leg={:?} error={error}",
                    self.config.strategy_id,
                    leg,
                );
            }
        }
        #[cfg(test)]
        let _ = &client_order_id;
    }

    fn cancel_all(&mut self, order_side: Option<OrderSide>) {
        #[cfg(not(test))]
        {
            for instrument_id in [self.yes_instrument_id, self.no_instrument_id] {
                if let Err(error) =
                    self.cancel_all_orders(instrument_id, order_side, Some(self.client_id()), None)
                {
                    log::error!(
                        "binary_oracle_maker cancel_all failed: strategy_id={} instrument_id={} error={error}",
                        self.config.strategy_id,
                        instrument_id,
                    );
                }
            }
        }
        #[cfg(test)]
        let _ = order_side;
    }

    fn cancel_all_one_side(&mut self, leg: Leg, order_side: OrderSide) {
        let instrument_id = self.leg_instrument_id(leg);
        #[cfg(not(test))]
        {
            if let Err(error) = self.cancel_all_orders(
                instrument_id,
                Some(order_side),
                Some(self.client_id()),
                None,
            ) {
                log::error!(
                    "binary_oracle_maker cancel_all_one_side failed: strategy_id={} instrument_id={} error={error}",
                    self.config.strategy_id,
                    instrument_id,
                );
            }
        }
        #[cfg(test)]
        let _ = (instrument_id, order_side);
    }

    // -- order-event fence + lifecycle forwarding ---------------------------

    /// Build a [`VenueReport`] from an NT execution report and run it through the
    /// per-leg fence; only an admitted `Ok(LegEvent)` is forwarded into the pure
    /// `MarketQuote`. A `FenceReject` is dropped (fail-closed) and logged.
    fn ingest_venue_report(
        &mut self,
        leg: Leg,
        client_order_id: &ClientOrderId,
        kind: VenueReportKind,
        now_ms: u64,
    ) -> Option<MarketAction> {
        let generation = self
            .leg_slot(leg)
            .fence
            .expected()
            .map(OrderIdentity::generation)
            .unwrap_or(INITIAL_GENERATION);
        let report = VenueReport {
            client_order_id: FenceClientOrderId::new(client_order_id.to_string()),
            generation,
            kind,
        };
        match self.leg_slot(leg).fence.admit(&report) {
            Ok(event) => {
                self.apply_leg_state_side_effects(leg, event, now_ms);
                self.market.on_leg_event(leg, event)
            }
            Err(reject) => {
                self.log_fence_reject(leg, client_order_id, &reject);
                None
            }
        }
    }

    fn log_fence_reject(&self, leg: Leg, client_order_id: &ClientOrderId, reject: &FenceReject) {
        log::warn!(
            "binary_oracle_maker fence dropped a venue report (fail-closed): strategy_id={} leg={:?} client_order_id={} reject={:?}",
            self.config.strategy_id,
            leg,
            client_order_id,
            reject,
        );
    }

    /// Update the shell-owned slot state for an admitted leg event before the
    /// lifecycle machine consumes it (resting price, last-rest timestamp, fence
    /// clear on terminal events).
    fn apply_leg_state_side_effects(&mut self, leg: Leg, event: LegEvent, now_ms: u64) {
        match event {
            LegEvent::Accepted | LegEvent::Modified => {
                let slot = self.leg_slot_mut(leg);
                slot.resting_price = slot.pending_price.take().or(slot.resting_price);
                slot.last_rest_ms = Some(now_ms);
            }
            LegEvent::Filled | LegEvent::Rejected => {
                let slot = self.leg_slot_mut(leg);
                slot.fence.clear();
                slot.resting_price = None;
                slot.pending_price = None;
                slot.client_order_id = None;
                slot.last_rest_ms = None;
            }
            LegEvent::Canceled => {
                // A cancel may be a requote (resubmit follows) or a wind-down. The
                // lifecycle machine owns that distinction; the shell only clears the
                // resting price (the leg has no live resting order right now) — a
                // resubmit re-establishes pending/resting.
                let slot = self.leg_slot_mut(leg);
                slot.resting_price = None;
                slot.last_rest_ms = None;
            }
            LegEvent::CancelRejected | LegEvent::ModifyRejected | LegEvent::QuoteTrigger { .. } => {
            }
        }
    }

    /// Apply a confirmed fill to the inventory book. A binary maker rests bids on
    /// both legs, so a maker fill is a Buy on that leg.
    fn apply_fill_to_inventory(&mut self, leg: Leg, order_side: OrderSide, qty: f64) {
        let side = match order_side {
            OrderSide::Buy => QuoteSide::Buy,
            OrderSide::Sell => QuoteSide::Sell,
            _ => return,
        };
        let _ = self.inventory.apply_fill(leg, side, qty);
    }

    // -- ops cadence: stale-quote alarm + maintenance -----------------------

    fn run_ops_tick(&mut self, now_ms: u64) {
        let alarm = MarketStaleQuoteAlarm::evaluate(
            now_ms,
            self.yes_slot.last_rest_ms,
            self.no_slot.last_rest_ms,
            self.config.max_rest_age_ms,
        );
        if alarm.any_stale() {
            log::info!(
                "binary_oracle_maker stale-quote alarm: strategy_id={} stale_legs={}",
                self.config.strategy_id,
                alarm.stale_legs().len(),
            );
            for action in alarm.refresh_plan() {
                // Drive the cancel through the lifecycle machine so it remains the
                // single owner of order transitions, then translate to NT.
                if let MarketAction::Leg { leg, .. } = action
                    && let Some(lifecycle_action) = self.market.cancel_leg(leg)
                {
                    self.execute_action(lifecycle_action, None, now_ms);
                }
            }
        }
        // Maintenance gate is also consulted in the quote tick's governor fold;
        // the ops cadence additionally drains both legs inside / approaching a
        // configured window. With no window configured the gate is skipped (it
        // does not veto), matching `governor_state`.
        if let Some(maintenance_state) = self.maintenance_governor_state(now_ms)
            && let Some(action) = cancel_all_on_kill(maintenance_state, &mut self.market)
        {
            self.execute_action(action, None, now_ms);
        }
    }

    // -- reconnect data-gap watchdog ----------------------------------------

    /// NT fires NO reconnect callback (NEEDS-VERIFY Q2 resolved: reconnect is
    /// internal to the adapter). The shell self-detects a probable reconnect from
    /// a per-leg data gap: if the time since the last book/quote update on either
    /// leg exceeds the configured staleness threshold, treat it as a probable
    /// disconnect and reconcile believed-resting state against venue truth via
    /// `maker_resync` BEFORE re-arming. Under no-submit this is a dry run — the
    /// admit gate blocks any live cancel/submit the reconciliation emits.
    fn run_reconnect_watchdog(&mut self, now_ms: u64) {
        let gap_threshold = self.config.data_gap_staleness_ms;
        if gap_threshold == 0 {
            return;
        }
        let yes_gapped = data_gap_exceeds(self.yes_last_data_ms, now_ms, gap_threshold);
        let no_gapped = data_gap_exceeds(self.no_last_data_ms, now_ms, gap_threshold);
        if !(yes_gapped || no_gapped) {
            return;
        }
        log::warn!(
            "binary_oracle_maker data-gap watchdog tripped (probable reconnect): strategy_id={} yes_gapped={} no_gapped={}",
            self.config.strategy_id,
            yes_gapped,
            no_gapped,
        );
        // Build the per-leg reconcile snapshots. `believed_resting` is the shell's
        // local belief; `venue_reports_open` is venue truth read from the NT cache
        // (open orders the exec client/adapter re-synced on reconnect).
        let yes_snapshot = LegReconcileSnapshot::new(
            Leg::Yes,
            self.leg_slot(Leg::Yes).resting_price.is_some(),
            self.venue_reports_open(Leg::Yes),
        );
        let no_snapshot = LegReconcileSnapshot::new(
            Leg::No,
            self.leg_slot(Leg::No).resting_price.is_some(),
            self.venue_reports_open(Leg::No),
        );
        let Some(snapshot) = MarketReconcileSnapshot::new(yes_snapshot, no_snapshot) else {
            return;
        };
        for action in snapshot.reconcile() {
            let target = match action {
                MarketAction::Leg {
                    leg,
                    action: LifecycleAction::Submit,
                } => self.pending_target_for_resubmit(leg),
                _ => None,
            };
            self.execute_action(action, target, now_ms);
        }
        // Reset the gap clocks so a single trip does not re-fire every tick until
        // fresh data arrives.
        if yes_gapped {
            self.yes_last_data_ms = Some(now_ms);
        }
        if no_gapped {
            self.no_last_data_ms = Some(now_ms);
        }
    }

    /// Whether the venue reports an open order on `leg` after a reconnect, read
    /// from the NT cache by the leg's tracked client order id. In tests (no
    /// registered cache) this is conservatively `false`.
    fn venue_reports_open(&self, leg: Leg) -> bool {
        #[cfg(not(test))]
        {
            if let Some(client_order_id) = self.leg_slot(leg).client_order_id.as_ref()
                && self.is_registered()
            {
                return self
                    .cache()
                    .order(client_order_id)
                    .is_some_and(|order| order.is_open());
            }
            false
        }
        #[cfg(test)]
        {
            let _ = leg;
            false
        }
    }

    // -- settlement on resolution -------------------------------------------

    /// Settle the held YES/NO lots at a binary resolution. The shell reads NT's
    /// per-instrument position + `avg_px_open` for both outcome instruments,
    /// builds two [`TokenLot`]s from the SAME market, and calls [`settle`]. On the
    /// live path NT does NOT auto-book the 0/1 close (NEEDS-VERIFY Q1 resolved),
    /// so this is the sole source of the settlement payout.
    fn settle_on_resolution(&mut self, outcome: SettlementOutcome) {
        let yes_lot = self.token_lot_for_instrument(self.yes_instrument_id);
        let no_lot = self.token_lot_for_instrument(self.no_instrument_id);
        let (Some(yes_lot), Some(no_lot)) = (yes_lot, no_lot) else {
            return;
        };
        let Some(booking) = settle(yes_lot, no_lot, outcome) else {
            log::error!(
                "binary_oracle_maker settlement booking failed (degenerate lots): strategy_id={}",
                self.config.strategy_id,
            );
            return;
        };
        self.last_settlement_pnl = Some(booking.realized_pnl());
        log::info!(
            "binary_oracle_maker settled binary resolution: strategy_id={} outcome={:?} payout={} realized_pnl={}",
            self.config.strategy_id,
            outcome,
            booking.payout(),
            booking.realized_pnl(),
        );
        // Resolution flattens the maker: reset inventory.
        self.inventory = MakerInventory::flat();
    }

    /// Read NT's position + `avg_px_open` for `instrument_id` into a [`TokenLot`].
    /// `None` only on a poisoned read; production finds the position when held.
    fn token_lot_for_instrument(&self, instrument_id: InstrumentId) -> Option<TokenLot> {
        #[cfg(not(test))]
        {
            if !self.is_registered() {
                return Some(TokenLot::flat());
            }
            let strategy_id = StrategyId::from(self.config.strategy_id.as_str());
            let cache = self.cache();
            let positions =
                cache.positions(None, Some(&instrument_id), Some(&strategy_id), None, None);
            let Some(position) = positions.into_iter().next() else {
                return Some(TokenLot::flat());
            };
            let signed_position = match position.side {
                PositionSide::Long => position.quantity.as_f64(),
                PositionSide::Short => -position.quantity.as_f64(),
                _ => ZERO_F64,
            };
            Some(TokenLot::new(signed_position, position.avg_px_open))
        }
        #[cfg(test)]
        {
            let _ = instrument_id;
            Some(TokenLot::flat())
        }
    }

    /// Resolve the settled outcome for a closed position by instrument identity —
    /// NT owns the position; the shell only names the side that closed.
    fn settlement_outcome_for_close(
        &self,
        instrument_id: InstrumentId,
    ) -> Option<SettlementOutcome> {
        self.leg_for_instrument(instrument_id)
    }

    /// Touch the per-leg data-gap watchdog timestamp from a market-data event.
    fn touch_leg_data(&mut self, instrument_id: InstrumentId, now_ms: u64) {
        if instrument_id == self.yes_instrument_id {
            self.yes_last_data_ms = Some(now_ms);
        } else if instrument_id == self.no_instrument_id {
            self.no_last_data_ms = Some(now_ms);
        }
    }

    fn order_is_closed(&self, client_order_id: &ClientOrderId) -> bool {
        #[cfg(not(test))]
        {
            if self.is_registered() {
                return self
                    .cache()
                    .order(client_order_id)
                    .map(|order| order.is_closed())
                    .unwrap_or(true);
            }
            true
        }
        #[cfg(test)]
        {
            let _ = client_order_id;
            true
        }
    }

    /// The pending quote target for a requote-cancel resubmit, reconstructed from
    /// the slot's pending price (the desired price the cancel was emitted for).
    fn pending_target_for_resubmit(&self, leg: Leg) -> Option<QuoteTargetLeg> {
        self.leg_slot(leg)
            .pending_price
            .map(|price| QuoteTargetLeg {
                // Both binary legs rest bids.
                side: QuoteSide::Buy,
                price,
            })
    }

    /// Adopt NT's authoritative position into the maker inventory model (NT owns
    /// PnL/position). NT's per-leg signed position seeds the inventory book.
    fn adopt_position_into_inventory(
        &mut self,
        instrument_id: InstrumentId,
        side: PositionSide,
        quantity: f64,
    ) {
        let Some(leg) = self.leg_for_instrument(instrument_id) else {
            return;
        };
        let quote_side = match side {
            PositionSide::Long => QuoteSide::Buy,
            PositionSide::Short => QuoteSide::Sell,
            _ => return,
        };
        if !is_positive_finite(quantity) {
            return;
        }
        let _ = self.inventory.apply_fill(leg, quote_side, quantity);
    }
}

// ---------------------------------------------------------------------------
// NT lifecycle / data hooks (DataActor block — &event -> Result<()>)
// ---------------------------------------------------------------------------

impl DataActor for BinaryOracleMaker {
    fn on_start(&mut self) -> Result<()> {
        self.subscribe_reference_quotes();
        self.subscribe_both_legs();
        self.register_timers();
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        // Cancel resting quotes on both legs (a stop must not leak live quotes),
        // unsubscribe, deregister timers.
        self.cancel_all(None);
        self.unsubscribe_reference_quotes();
        self.unsubscribe_both_legs();
        self.deregister_timers();
        Ok(())
    }

    fn on_time_event(&mut self, event: &TimeEvent) -> Result<()> {
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        let name = event.name.as_str();
        if name == self.requote_timer_name() {
            // Reconnect watchdog runs first so a probable reconnect reconciles
            // before any fresh quoting this tick.
            self.run_reconnect_watchdog(now_ms);
            self.run_quote_tick(now_ms);
        } else if name == self.ops_timer_name() {
            self.run_ops_tick(now_ms);
        }
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> Result<()> {
        // Drive the maker spot / realized-vol feed from the reference instrument.
        if self
            .reference_instrument_id_parsed
            .is_none_or(|instrument_id| quote.instrument_id != instrument_id)
        {
            return Ok(());
        }
        let bid = quote.bid_price.as_f64();
        let ask = quote.ask_price.as_f64();
        let now_ms = quote.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        if is_positive_finite(bid) && is_positive_finite(ask) {
            self.spot.observe((bid + ask) / TWO_F64, now_ms);
        }
        Ok(())
    }

    fn on_book_deltas(&mut self, deltas: &OrderBookDeltas) -> Result<()> {
        let Some(leg) = self.leg_for_instrument(deltas.instrument_id) else {
            return Ok(());
        };
        let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
        match leg {
            Leg::Yes => self.yes_book.update_from_deltas(deltas),
            Leg::No => self.no_book.update_from_deltas(deltas),
        }
        self.touch_leg_data(deltas.instrument_id, now_ms);
        // A book move is a reprice trigger alongside the requote timer.
        self.run_quote_tick(now_ms);
        Ok(())
    }

    fn on_trade(&mut self, trade: &TradeTick) -> Result<()> {
        if let Some(trade_flow) = self.trade_flow.get_mut(&trade.instrument_id) {
            trade_flow.observe(trade);
        }
        let now_ms = trade.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        self.touch_leg_data(trade.instrument_id, now_ms);
        Ok(())
    }

    fn on_order_filled(&mut self, event: &OrderFilled) -> Result<()> {
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        let Some(leg) = self
            .leg_for_client_order_id(&event.client_order_id)
            .or_else(|| self.leg_for_instrument(event.instrument_id))
        else {
            return Ok(());
        };
        // A partial fill (remainder still working) is not a lifecycle Filled
        // event; only a fill that leaves zero quantity working is. NT's
        // OrderFilled carries cumulative state — gate on the order being closed in
        // the cache (a fully-filled order is closed); treat an order no longer in
        // the cache as a full fill.
        let fully_filled = self.order_is_closed(&event.client_order_id);
        self.apply_fill_to_inventory(leg, event.order_side, event.last_qty.as_f64());
        if fully_filled
            && let Some(action) = self.ingest_venue_report(
                leg,
                &event.client_order_id,
                VenueReportKind::Filled,
                now_ms,
            )
        {
            self.execute_action(action, None, now_ms);
        }
        Ok(())
    }

    fn on_order_canceled(&mut self, event: &OrderCanceled) -> Result<()> {
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        if let Some(leg) = self.leg_for_client_order_id(&event.client_order_id)
            && let Some(action) = self.ingest_venue_report(
                leg,
                &event.client_order_id,
                VenueReportKind::Canceled,
                now_ms,
            )
        {
            // A confirmed requote cancel emits the replacement Submit; drive the
            // resubmit through the slot's pending price when present.
            let target = self.pending_target_for_resubmit(leg);
            self.execute_action(action, target, now_ms);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// NT by-value Strategy order/position hooks (nautilus_strategy! macro block)
// ---------------------------------------------------------------------------

nautilus_strategy!(BinaryOracleMaker, {
    fn on_order_rejected(&mut self, event: nautilus_model::events::OrderRejected) {
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        if let Some(leg) = self.leg_for_client_order_id(&event.client_order_id)
            && let Some(action) = self.ingest_venue_report(
                leg,
                &event.client_order_id,
                VenueReportKind::Rejected,
                now_ms,
            )
        {
            self.execute_action(action, None, now_ms);
        }
    }

    fn on_order_expired(&mut self, event: nautilus_model::events::OrderExpired) {
        // A GTD/short-TTL quote expiry frees the slot; treat it as a cancel (the
        // order left the book without a fill) so the requote loop replaces it.
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        if let Some(leg) = self.leg_for_client_order_id(&event.client_order_id)
            && let Some(action) = self.ingest_venue_report(
                leg,
                &event.client_order_id,
                VenueReportKind::Canceled,
                now_ms,
            )
        {
            self.execute_action(action, None, now_ms);
        }
    }

    fn on_position_opened(&mut self, _event: nautilus_model::events::PositionOpened) {
        self.adopt_position_into_inventory(
            _event.instrument_id,
            _event.side,
            _event.quantity.as_f64(),
        );
    }

    fn on_position_changed(&mut self, _event: nautilus_model::events::PositionChanged) {
        self.adopt_position_into_inventory(
            _event.instrument_id,
            _event.side,
            _event.quantity.as_f64(),
        );
    }

    fn on_position_closed(&mut self, event: nautilus_model::events::PositionClosed) {
        // Settlement close-seam: NT delivers the binary resolution as a position
        // close. Settle the held YES/NO lots here (NT does not auto-book the live
        // 0/1 payout). See module docs + report for the seam justification.
        if let Some(outcome) = self.settlement_outcome_for_close(event.instrument_id) {
            self.settle_on_resolution(outcome);
        }
    }
});

// ---------------------------------------------------------------------------
// Builder + registration
// ---------------------------------------------------------------------------

/// Strategy builder that registers the no-submit maker shell into the runtime.
#[derive(Debug)]
pub struct BinaryOracleMakerBuilder;

impl BinaryOracleMakerBuilder {
    fn parse_config(raw: &Value) -> Result<BinaryOracleMakerConfig> {
        let config: BinaryOracleMakerConfig = raw
            .clone()
            .try_into()
            .context("binary_oracle_maker config failed to deserialize")?;
        Ok(config)
    }
}

impl StrategyBuilder for BinaryOracleMakerBuilder {
    fn kind() -> &'static str {
        KEY
    }

    fn validate_config(raw: &Value, field_prefix: &str, errors: &mut Vec<ValidationError>) {
        let config = match Self::parse_config(raw) {
            Ok(config) => config,
            Err(error) => {
                errors.push(ValidationError {
                    field: field_prefix.to_string(),
                    code: DESERIALIZE_FAILED_CODE,
                    message: format!("{error:#}"),
                });
                return;
            }
        };
        // Fail-loud: surface EVERY parameter-domain error (collect-all).
        let context_label = format!("{field_prefix}.parameters");
        if let Err(messages) = config.parameters.validate(&context_label) {
            for message in messages {
                errors.push(ValidationError {
                    field: format!("{field_prefix}.parameters"),
                    code: PARAMETER_OUT_OF_DOMAIN_CODE,
                    message,
                });
            }
        }
    }

    fn build(raw: &Value, context: &StrategyBuildContext) -> Result<BoxedStrategy> {
        Ok(Box::new(BinaryOracleMaker::new(
            Self::parse_config(raw)?,
            context.clone(),
        )))
    }

    fn register(
        raw: &Value,
        context: &StrategyBuildContext,
        trader: &Rc<RefCell<Trader>>,
    ) -> Result<StrategyId> {
        let strategy = BinaryOracleMaker::new(Self::parse_config(raw)?, context.clone());
        let strategy_id = StrategyId::from(strategy.component_id().inner().as_str());
        trader.borrow_mut().add_strategy(strategy)?;
        Ok(strategy_id)
    }
}

// Tests are kept INLINE in a braced `#[cfg(test)] mod tests { ... }` (rather than
// a sibling file) so the runtime-literal verifier strips the whole module — its
// fixture literals are test-only and must not be scanned as runtime values
// (mirrors the taker's inline-test convention).
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::Result;
    use futures_util::future::{BoxFuture, FutureExt};
    use nautilus_model::enums::OrderSide;
    use nautilus_model::identifiers::{ClientOrderId, InstrumentId};
    use rust_decimal::Decimal;
    use toml::Value;

    use super::*;
    use crate::bolt_v3_decision_evidence::{
        BoltV3AdmissionDecisionEvidence, BoltV3DecisionEvidenceWriter, BoltV3OrderIntentEvidence,
        BoltV3StrategyInputEvidenceSnapshot,
    };
    use crate::bolt_v3_submit_admission::{
        BoltV3SubmitAdmissionError, BoltV3SubmitAdmissionRequest, BoltV3SubmitAdmissionState,
        BoltV3SubmitIntentKind, BoltV3SubmitLifecyclePolicy,
    };
    use crate::strategies::maker_event_fence::{
        ClientOrderId as FenceClientOrderId, ExpectedIdentity, OrderIdentity, VenueReport,
        VenueReportKind,
    };
    use crate::strategies::maker_governor::MakerGovernorState;
    use crate::strategies::quote_lifecycle::{Leg, LegState};
    use crate::strategies::registry::{
        FeeProvider, StrategyBuildContext, StrategyBuilder, ValidationError,
    };

    // -- noop providers (mirror the registry test fixtures) -----------------

    #[derive(Debug, Clone)]
    struct NoopFeeProvider;

    impl FeeProvider for NoopFeeProvider {
        fn fee_bps(&self, _instrument_id: InstrumentId) -> Option<Decimal> {
            None
        }

        fn warm(&self, _instrument_id: InstrumentId) -> BoxFuture<'_, Result<()>> {
            async { Ok(()) }.boxed()
        }
    }

    #[derive(Debug)]
    struct NoopDecisionEvidenceWriter;

    impl BoltV3DecisionEvidenceWriter for NoopDecisionEvidenceWriter {
        fn record_strategy_input_snapshot(
            &self,
            _snapshot: &BoltV3StrategyInputEvidenceSnapshot,
        ) -> Result<()> {
            Ok(())
        }

        fn record_order_intent(&self, _intent: &BoltV3OrderIntentEvidence) -> Result<()> {
            Ok(())
        }

        fn record_admission_decision(
            &self,
            _decision: &BoltV3AdmissionDecisionEvidence,
        ) -> Result<()> {
            Ok(())
        }
    }

    fn unarmed_admission() -> Arc<BoltV3SubmitAdmissionState> {
        Arc::new(BoltV3SubmitAdmissionState::new_unarmed(Arc::new(
            NoopDecisionEvidenceWriter,
        )))
    }

    fn test_context_with(admission: Arc<BoltV3SubmitAdmissionState>) -> StrategyBuildContext {
        StrategyBuildContext::new(
            Arc::new(NoopFeeProvider),
            Arc::new(NoopDecisionEvidenceWriter),
            admission,
            nautilus_model::identifiers::Venue::from("POLYMARKET"),
        )
    }

    /// A full, valid maker config. Literals here are test-only (the
    /// runtime-literal verifier strips this `#[cfg(test)] mod tests` block).
    fn valid_config_toml() -> &'static str {
        r#"
strategy_id = "MAKER-001"
order_id_tag = "001"
client_id = "POLYMARKET"
family_key = "updown"
strike_price = 100.0
reference_instrument_id = "BTCUSDT-PERP.BINANCE"
reference_venue = "BINANCE"
yes_instrument_id = "YESTOKEN.POLYMARKET"
no_instrument_id = "NOTOKEN.POLYMARKET"
quote_size = 5.0
supports_modify = false
requote_cadence_seconds = 1
ops_cadence_seconds = 5
requote_max_cost_per_window = 100
requote_window_ms = 60000
max_rest_age_ms = 30000
data_gap_staleness_ms = 10000
maintenance_window_start_ms = 0
maintenance_window_duration_ms = 0
maintenance_pre_flatten_lead_ms = 0
requote_threshold = 0.001
pricing_kurtosis = 0.0
vol_window_secs = 600
vol_gap_reset_secs = 120
vol_min_observations = 3
vol_bridge_valid_secs = 600
trade_flow_window_secs = 300
trade_flow_max_samples = 256

[parameters]
eps = 0.01
reference_tau = 3600.0
time_widen_cap = 4.0
micro_weight = 0.3
sigma_floor = 0.0
basis_cap = 1.0
tau_floor = 0.0
reduce_only_cap = 100.0
skew_gain = 0.001
position_cap = 500.0
half_spread_floor = 0.005
max_half_spread = 0.2
requote_min_interval_ms = 0
informed_fraction = 0.25
"#
    }

    fn valid_config_value() -> Value {
        toml::from_str::<Value>(valid_config_toml()).expect("valid maker config must parse")
    }

    fn make_maker(admission: Arc<BoltV3SubmitAdmissionState>) -> BinaryOracleMaker {
        let config = BinaryOracleMakerBuilder::parse_config(&valid_config_value())
            .expect("valid config must deserialize");
        BinaryOracleMaker::new(config, test_context_with(admission))
    }

    /// Feed enough reference spot observations to make the realized-vol estimator
    /// ready AND seed a YES-leg touch so the governor reads a finite venue mid
    /// near the oracle fair (otherwise the basis is NaN -> HardFlat). Returns the
    /// `now_ms` at which a fair value should be available.
    fn warm_spot_feed(maker: &mut BinaryOracleMaker) -> u64 {
        let base = 100.0_f64;
        let mut ts = 1_000_u64;
        for i in 0..8u64 {
            let price = base + if i % 2 == 0 { 0.5 } else { -0.5 };
            maker.spot.observe(price, ts);
            ts += 1_000;
        }
        // Seed a YES-leg touch straddling ~0.5 (the spot==strike fair) so the
        // governor's |oracle_fair - venue_mid| basis stays well inside basis_cap.
        maker.yes_book.best_bid = Some(0.49);
        maker.yes_book.best_bid_size = Some(10.0);
        maker.yes_book.best_ask = Some(0.51);
        maker.yes_book.best_ask_size = Some(10.0);
        ts
    }

    // 1. Config validation ---------------------------------------------------

    #[test]
    fn a_valid_config_validates_into_a_validated_maker_config() {
        let config = BinaryOracleMakerBuilder::parse_config(&valid_config_value())
            .expect("valid config deserializes");
        let validated = config
            .parameters
            .validate("ctx")
            .expect("valid parameters validate");
        assert_eq!(validated.eps(), 0.01);
        assert_eq!(validated.informed_fraction(), 0.25);
        assert_eq!(validated.position_cap(), 500.0);
    }

    #[test]
    fn an_invalid_parameter_block_fails_loud_through_the_builder() {
        let toml = valid_config_toml().replace("eps = 0.01", "eps = 0.5");
        let raw = toml::from_str::<Value>(&toml).expect("config still parses");
        let mut errors: Vec<ValidationError> = Vec::new();
        BinaryOracleMakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
        assert!(
            errors.iter().any(|e| e.message.contains("eps")),
            "expected a fail-loud eps domain error, got: {errors:?}"
        );
    }

    #[test]
    fn an_unknown_field_is_a_loud_deserialize_error() {
        let mut toml = valid_config_toml().to_string();
        toml.push_str("not_a_real_field = 1\n");
        let raw = toml::from_str::<Value>(&toml).expect("toml parses");
        let result = BinaryOracleMakerBuilder::parse_config(&raw);
        assert!(
            result.is_err(),
            "deny_unknown_fields must reject an unrecognized key"
        );
    }

    // 2. Builder registration ------------------------------------------------

    #[test]
    fn the_maker_builder_kind_is_distinct_from_the_taker() {
        assert_eq!(BinaryOracleMakerBuilder::kind(), "binary_oracle_maker");
        assert_ne!(
            BinaryOracleMakerBuilder::kind(),
            crate::strategies::binary_oracle_edge_taker::BinaryOracleEdgeTakerBuilder::kind(),
            "maker kind must not collide with the taker kind"
        );
    }

    #[test]
    fn the_production_registry_includes_the_maker() {
        let registry =
            crate::strategies::production_strategy_registry().expect("registry builds without dup");
        assert!(
            registry.kinds().contains(&"binary_oracle_maker"),
            "production registry must register the maker shell: {:?}",
            registry.kinds()
        );
        assert!(
            registry.kinds().contains(&"binary_oracle_edge_taker"),
            "the taker must remain registered alongside the maker"
        );
    }

    // 3. Quote-loop MarketAction -> intent translation -----------------------

    #[test]
    fn a_representative_tick_translates_both_legs_into_submit_attempts() {
        let mut maker = make_maker(unarmed_admission());
        let now_ms = warm_spot_feed(&mut maker);

        let fair = maker
            .anchored_fair(now_ms)
            .expect("warm spot/RV must yield an oracle fair");
        assert!(maker.governor_state(now_ms, fair) == MakerGovernorState::Quoting);
        let targets = maker
            .desired_quote_targets(fair)
            .expect("a healthy tick must produce quote targets");
        assert!(targets.leg_a.price > 0.0 && targets.leg_a.price < 1.0);
        assert!(targets.leg_b.price > 0.0 && targets.leg_b.price < 1.0);

        maker.run_quote_tick(now_ms);
        assert_eq!(
            maker.submit_attempts.len(),
            2,
            "an idle-both-legs quoting tick must translate into two leg submits"
        );
        assert_eq!(maker.market.leg_state(Leg::Yes), LegState::SubmitPending);
        assert_eq!(maker.market.leg_state(Leg::No), LegState::SubmitPending);
    }

    #[test]
    fn no_oracle_fair_skips_quoting_fail_closed() {
        let mut maker = make_maker(unarmed_admission());
        maker.run_quote_tick(1_000);
        assert!(
            maker.submit_attempts.is_empty(),
            "a tick with no oracle fair must emit no quotes (fail-closed)"
        );
        assert_eq!(maker.market.leg_state(Leg::Yes), LegState::Idle);
    }

    // 4. Event fence drops a foreign / stale report --------------------------

    #[test]
    fn the_fence_drops_a_foreign_client_id_report() {
        let expected = ExpectedIdentity::submitting(OrderIdentity::new(
            FenceClientOrderId::new("real-1".to_string()),
            0,
        ));
        let foreign = VenueReport {
            client_order_id: FenceClientOrderId::new("foreign-9".to_string()),
            generation: 0,
            kind: VenueReportKind::Accepted,
        };
        assert!(
            expected.admit(&foreign).is_err(),
            "a foreign client id must be rejected by the fence"
        );
    }

    #[test]
    fn the_fence_drops_a_stale_generation_report() {
        let mut maker = make_maker(unarmed_admission());
        let now_ms = warm_spot_feed(&mut maker);
        maker.run_quote_tick(now_ms);
        let client_order_id = maker
            .leg_slot(Leg::Yes)
            .client_order_id
            .expect("a YES submit attempt must have stamped a client order id");
        assert_eq!(maker.market.leg_state(Leg::Yes), LegState::SubmitPending);

        // Forge a STALE report: right client id, older generation than expected.
        let stale_identity =
            OrderIdentity::new(FenceClientOrderId::new(client_order_id.to_string()), 5);
        maker.yes_slot.fence.requote_to(stale_identity);
        let stale_report = VenueReport {
            client_order_id: FenceClientOrderId::new(client_order_id.to_string()),
            generation: 1, // < 5 expected => StaleGeneration
            kind: VenueReportKind::Filled,
        };
        assert!(
            maker.yes_slot.fence.admit(&stale_report).is_err(),
            "a stale-generation report must be rejected"
        );

        // And through the shell's ingest path: a genuinely foreign client id is
        // dropped (fail-closed) and produces no MarketAction.
        let foreign = maker.ingest_venue_report(
            Leg::Yes,
            &ClientOrderId::from("totally-foreign"),
            VenueReportKind::Filled,
            now_ms,
        );
        assert!(
            foreign.is_none(),
            "a foreign report must produce no MarketAction (dropped fail-closed)"
        );
    }

    // 5. No-submit admit gate ------------------------------------------------

    #[test]
    fn the_admit_gate_rejects_a_submit_while_unarmed() {
        let admission = unarmed_admission();
        let request = BoltV3SubmitAdmissionRequest {
            strategy_id: "MAKER-001".to_string(),
            client_order_id: "co-1".to_string(),
            instrument_id: "YESTOKEN.POLYMARKET".to_string(),
            notional: Decimal::new(5, 0),
            intent_kind: BoltV3SubmitIntentKind::Entry,
            lifecycle_policy: BoltV3SubmitLifecyclePolicy::new(false),
            canary_proof_claim: None,
        };
        let outcome = admission.admit(&request);
        assert!(
            matches!(outcome, Err(BoltV3SubmitAdmissionError::NotArmed)),
            "an unarmed admission state must reject every order with NotArmed, got: {outcome:?}"
        );
        assert_eq!(admission.admitted_order_count(), 0);
    }

    #[test]
    fn no_live_order_escapes_the_quote_loop_while_unarmed() {
        let admission = unarmed_admission();
        let mut maker = make_maker(admission.clone());
        let now_ms = warm_spot_feed(&mut maker);
        maker.run_quote_tick(now_ms);
        assert_eq!(maker.submit_attempts.len(), 2);
        assert!(
            maker
                .submit_attempts
                .iter()
                .all(|attempt| !attempt.admitted),
            "no submit may be admitted while the shared admission state is unarmed: {:?}",
            maker.submit_attempts
        );
        assert_eq!(
            admission.admitted_order_count(),
            0,
            "the unarmed gate must keep the admitted-order count at zero"
        );
    }

    // 6. Inventory adoption from fills ---------------------------------------

    #[test]
    fn a_confirmed_yes_buy_fill_lengthens_the_inventory() {
        let mut maker = make_maker(unarmed_admission());
        maker.apply_fill_to_inventory(Leg::Yes, OrderSide::Buy, 3.0);
        assert!((maker.inventory.net_position() - 3.0).abs() < 1e-9);
        maker.apply_fill_to_inventory(Leg::No, OrderSide::Buy, 1.0);
        assert!((maker.inventory.net_position() - 2.0).abs() < 1e-9);
    }
}
