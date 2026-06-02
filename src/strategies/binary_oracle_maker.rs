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
        portfolio_selection::{MarketCandidate, MarketKey, SelectionWeights},
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
/// Validation error code: the `[[markets]]` list is empty, has a duplicate
/// market id, or carries an unparseable / non-unique outcome instrument id.
const MARKETS_INVALID_CODE: &str = stringify!(markets_invalid);
/// Validation error code: the `[selection]` block is degenerate — its weights
/// were rejected by `SelectionWeights::new` or `max_active_markets` is below 1.
const SELECTION_INVALID_CODE: &str = stringify!(selection_invalid);
/// The minimum `max_active_markets` value: a portfolio must admit at least one
/// active market for selection to mean anything (zero would quote nothing,
/// silently). No bare literal on the runtime path — the floor is a named const.
const MIN_MAX_ACTIVE_MARKETS: u64 = 1;
/// Diagnostic label for the YES outcome instrument field, named from the field
/// ident itself so the label cannot drift from the config field name.
const YES_INSTRUMENT_LABEL: &str = stringify!(yes_instrument_id);
/// Diagnostic label for the NO outcome instrument field, named from the field
/// ident itself so the label cannot drift from the config field name.
const NO_INSTRUMENT_LABEL: &str = stringify!(no_instrument_id);

// ---------------------------------------------------------------------------
// TOML configuration
// ---------------------------------------------------------------------------

/// One binary market the maker quotes (FR-041 multi-market entry).
///
/// Carries the per-market identity (`market_id`), the two outcome instruments,
/// the binary strike fed to the family fair-value model, and the per-market
/// maintenance window. A `[[markets]]` TOML array of these is the single source
/// of truth for the set of markets the shell runs — single-market is just a
/// list of length one (NO DUAL PATHS). Deserialized with `deny_unknown_fields`
/// so a stale/misspelled per-market knob is a loud parse error, matching the
/// parent block's contract.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakerMarketConfig {
    /// Stable per-market identity, keyed into the per-market state map and the
    /// market-unique client order id. Non-empty and unique across the list.
    pub market_id: String,

    /// YES (leg-a / "up") outcome instrument id.
    pub yes_instrument_id: String,
    /// NO (leg-b / "down") outcome instrument id.
    pub no_instrument_id: String,

    /// Binary strike price fed to the family fair-value model.
    pub strike_price: f64,

    /// Maintenance window start (ms epoch). `window_duration_ms == 0` disables.
    pub maintenance_window_start_ms: u64,
    /// Maintenance window duration in ms (0 = no maintenance window configured).
    pub maintenance_window_duration_ms: u64,
    /// Pre-flatten lead-up before the maintenance window (ms).
    pub maintenance_pre_flatten_lead_ms: u64,
}

/// Strategy-wide market-selection knobs (FR-041 market-SELECTION layer).
///
/// The maker scores its configured markets each quote tick via
/// [`crate::strategies::portfolio_selection`] and quotes only the most
/// attractive `max_active_markets`; the rest are not quoted and their resting
/// quotes are canceled. The three weights are the relative importance of the
/// three selection features (captured spread, top-of-book liquidity, time to
/// resolution) and are fed verbatim into
/// [`SelectionWeights::new`](crate::strategies::portfolio_selection::SelectionWeights::new)
/// — the single source of truth for their domain (every weight finite and
/// `>= 0`, at least one strictly positive). A single `[selection]` table on the
/// strategy config (NOT per-market — selection is a portfolio-wide decision).
/// Deserialized with `deny_unknown_fields` so a stale/misspelled knob is a loud
/// parse error, matching the parent block's contract.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakerSelectionConfig {
    /// Relative weight of the YES-leg captured spread (wider = more edge).
    pub spread_weight: f64,
    /// Relative weight of the YES-leg top-of-book liquidity (deeper = safer).
    pub liquidity_weight: f64,
    /// Relative weight of the seconds-to-resolution (more time = more requote
    /// cycles before the coin-flip).
    pub tau_weight: f64,
    /// Maximum number of markets quoted concurrently each tick: the top-K of the
    /// ranked active set. Must be `>= 1` (zero would quote nothing).
    pub max_active_markets: u64,
}

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

    /// Reference (oracle) instrument id whose quotes drive spot + realized vol.
    pub reference_instrument_id: String,
    /// Reference venue label keyed into the realized-vol estimator.
    pub reference_venue: String,

    /// The binary markets this maker quotes concurrently (FR-041 multi-market).
    /// A single-market deployment is simply a `[[markets]]` list of length one —
    /// there is NO separate single-market path. Each entry owns its own outcome
    /// instruments, strike, and maintenance window; everything else (pricing
    /// knobs, oracle feed, cadences, throttle budget) is shared across markets.
    pub markets: Vec<MakerMarketConfig>,

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

    /// The market-selection weights + top-K cap (nested `[selection]`). Drives
    /// the per-tick rank that decides which configured markets are quoted.
    pub selection: MakerSelectionConfig,
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

    /// Validate the `[[markets]]` list, fail-loud and collecting **all** errors
    /// (one human-readable line per violation, prefixed with `context`). The list
    /// must be non-empty; every `market_id` must be non-empty and unique; every
    /// outcome instrument id must parse to an [`InstrumentId`]; and every
    /// instrument id must be globally unique across ALL markets (so no instrument
    /// appears in two markets and `yes != no` within a market). On success the
    /// returned `Vec` is empty.
    ///
    /// Per-market strike / maintenance fields carry no further domain check here:
    /// `strike_price` is consumed by the family fair-value seam (which fails
    /// closed on a non-finite/degenerate value), and the maintenance windows are
    /// `u64` (non-negative by type; `duration == 0` is the documented "no window"
    /// sentinel) — the same domains the single-market fields carried, now per
    /// market.
    fn validate_markets(&self, context: &str) -> Vec<String> {
        let mut errors: Vec<String> = Vec::new();
        if self.markets.is_empty() {
            errors.push(format!(
                "{context}: markets must contain at least one [[markets]] entry"
            ));
            return errors;
        }
        let mut seen_market_ids: std::collections::BTreeSet<&str> =
            std::collections::BTreeSet::new();
        // Maps each instrument id string to the market_id that first claimed it,
        // so a collision names both markets.
        let mut seen_instruments: BTreeMap<&str, &str> = BTreeMap::new();
        for market in &self.markets {
            if market.market_id.is_empty() {
                errors.push(format!(
                    "{context}: every [[markets]] entry must have a non-empty market_id"
                ));
            } else if !seen_market_ids.insert(market.market_id.as_str()) {
                errors.push(format!(
                    "{context}: duplicate market_id `{}` — every [[markets]] entry must be unique",
                    market.market_id
                ));
            }
            for (label, raw_id) in [
                (YES_INSTRUMENT_LABEL, market.yes_instrument_id.as_str()),
                (NO_INSTRUMENT_LABEL, market.no_instrument_id.as_str()),
            ] {
                if InstrumentId::from_str(raw_id).is_err() {
                    errors.push(format!(
                        "{context}: market `{}` {label} `{raw_id}` is not a parseable instrument id",
                        market.market_id
                    ));
                    continue;
                }
                match seen_instruments.insert(raw_id, market.market_id.as_str()) {
                    None => {}
                    Some(prior_market_id) => {
                        errors.push(format!(
                            "{context}: instrument id `{raw_id}` ({label}) is used by more than one market (`{prior_market_id}` and `{}`) — every outcome instrument must be globally unique (and yes != no within a market)",
                            market.market_id
                        ));
                    }
                }
            }
        }
        errors
    }

    /// Validate the `[selection]` block, fail-loud and collecting **all** errors
    /// (one line per violation, prefixed with `context`). The weights are checked
    /// by **constructing** the pure
    /// [`SelectionWeights`](crate::strategies::portfolio_selection::SelectionWeights)
    /// via its guarded `new()` — the single source of truth for the weight domain
    /// (every weight finite and `>= 0`, at least one strictly positive); a `None`
    /// is one collected error. `max_active_markets` must be `>= 1` (a top-K of
    /// zero would silently quote nothing). On success the returned `Vec` is empty.
    fn validate_selection(&self, context: &str) -> Vec<String> {
        let mut errors: Vec<String> = Vec::new();
        if self.validated_selection_weights().is_none() {
            errors.push(format!(
                "{context}: weights rejected by SelectionWeights::new — spread_weight (`{}`), liquidity_weight (`{}`), tau_weight (`{}`) must each be finite and >= 0 with at least one strictly positive",
                self.selection.spread_weight,
                self.selection.liquidity_weight,
                self.selection.tau_weight,
            ));
        }
        if self.selection.max_active_markets < MIN_MAX_ACTIVE_MARKETS {
            errors.push(format!(
                "{context}: max_active_markets must be >= {MIN_MAX_ACTIVE_MARKETS}: `{}`",
                self.selection.max_active_markets
            ));
        }
        errors
    }

    /// Build the validated [`SelectionWeights`] from the `[selection]` block, or
    /// `None` if the weight set is degenerate. The single construction site for
    /// the weights — `validate_selection` checks it loud, and `new()` stores the
    /// `Some` so the tick path never rebuilds/revalidates.
    fn validated_selection_weights(&self) -> Option<SelectionWeights> {
        SelectionWeights::new(
            self.selection.spread_weight,
            self.selection.liquidity_weight,
            self.selection.tau_weight,
        )
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
// Per-market state unit (FR-041 multi-market)
// ---------------------------------------------------------------------------

/// All per-market state plus the per-market config values for ONE binary market.
///
/// FR-041 runs N concurrent thin binary markets off one shared bankroll, oracle
/// feed, and pricing-knob set. Everything that is per-market — the two outcome
/// instrument ids, the strike, the maintenance window, the lifecycle/governor/
/// inventory state, both leg books and slots, the per-leg signed-trade-flow
/// buffers, and the data-gap clocks — lives here, keyed by
/// [`MarketKey`](crate::strategies::portfolio_selection::MarketKey) on the
/// strategy. The shared spot feed, family, validated knobs, throttle config, and
/// admission gate stay on [`BinaryOracleMaker`]. There is no `Default`: a unit is
/// only ever built from a validated [`MakerMarketConfig`] via
/// [`MarketUnit::from_config`], so an all-zero unit cannot silently exist.
struct MarketUnit {
    yes_instrument_id: InstrumentId,
    no_instrument_id: InstrumentId,
    strike_price: f64,
    maintenance_window_start_ms: u64,
    maintenance_window_duration_ms: u64,
    maintenance_pre_flatten_lead_ms: u64,
    market: MarketQuote,
    governor: MakerGovernor,
    inventory: MakerInventory,
    requote_budget: RequoteBudget,
    yes_book: MakerLegBook,
    no_book: MakerLegBook,
    yes_slot: LegSlot,
    no_slot: LegSlot,
    yes_trade_flow: SignedTradeFlow,
    no_trade_flow: SignedTradeFlow,
    yes_last_data_ms: Option<u64>,
    no_last_data_ms: Option<u64>,
    /// Last settlement booking PnL, retained for observability/tests.
    last_settlement_pnl: Option<f64>,
}

impl MarketUnit {
    /// Build a fresh per-market unit from its validated [`MakerMarketConfig`] and
    /// the shared inputs it needs (the same values the old single-market `new()`
    /// initialized the moved fields from). Instrument ids are parsed here from the
    /// per-market config — `validate_config` already proved they parse.
    fn from_config(
        market_cfg: &MakerMarketConfig,
        kill_thresholds: crate::strategies::maker_governor::KillThresholds,
        requote_budget: RequoteBudget,
        supports_modify: bool,
        flow_config: &SignedTradeFlowConfig,
    ) -> Self {
        Self {
            yes_instrument_id: InstrumentId::from(market_cfg.yes_instrument_id.as_str()),
            no_instrument_id: InstrumentId::from(market_cfg.no_instrument_id.as_str()),
            strike_price: market_cfg.strike_price,
            maintenance_window_start_ms: market_cfg.maintenance_window_start_ms,
            maintenance_window_duration_ms: market_cfg.maintenance_window_duration_ms,
            maintenance_pre_flatten_lead_ms: market_cfg.maintenance_pre_flatten_lead_ms,
            market: MarketQuote::new(supports_modify),
            governor: MakerGovernor::new(kill_thresholds),
            inventory: MakerInventory::flat(),
            requote_budget,
            yes_book: MakerLegBook::empty(),
            no_book: MakerLegBook::empty(),
            yes_slot: LegSlot::idle(),
            no_slot: LegSlot::idle(),
            yes_trade_flow: SignedTradeFlow::from_config(flow_config),
            no_trade_flow: SignedTradeFlow::from_config(flow_config),
            yes_last_data_ms: None,
            no_last_data_ms: None,
            last_settlement_pnl: None,
        }
    }

    fn instrument_id_for(&self, leg: Leg) -> InstrumentId {
        match leg {
            Leg::Yes => self.yes_instrument_id,
            Leg::No => self.no_instrument_id,
        }
    }

    fn book(&self, leg: Leg) -> &MakerLegBook {
        match leg {
            Leg::Yes => &self.yes_book,
            Leg::No => &self.no_book,
        }
    }

    fn slot(&self, leg: Leg) -> &LegSlot {
        match leg {
            Leg::Yes => &self.yes_slot,
            Leg::No => &self.no_slot,
        }
    }

    fn slot_mut(&mut self, leg: Leg) -> &mut LegSlot {
        match leg {
            Leg::Yes => &mut self.yes_slot,
            Leg::No => &mut self.no_slot,
        }
    }

    fn trade_flow_mut(&mut self, leg: Leg) -> &mut SignedTradeFlow {
        match leg {
            Leg::Yes => &mut self.yes_trade_flow,
            Leg::No => &mut self.no_trade_flow,
        }
    }

    /// Whether this unit's `leg` slot tracks `client_order_id` as its live order.
    fn slot_tracks(&self, leg: Leg, client_order_id: &ClientOrderId) -> bool {
        self.slot(leg)
            .client_order_id
            .as_ref()
            .is_some_and(|id| id == client_order_id)
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

    reference_instrument_id_parsed: Option<InstrumentId>,

    family: BinaryFamily,
    spot: MakerSpotFeed,

    /// The validated market-selection weights, built once in `new()` from the
    /// `[selection]` block (the builder's `validate_config` already proved they
    /// construct), so the per-tick rank never rebuilds/revalidates them.
    selection_weights: SelectionWeights,
    /// The top-K cap on the active market set, validated `>= 1`.
    max_active_markets: u64,

    /// Per-market state, keyed by the market's stable id. The single source of
    /// truth for which markets the shell runs is `config.markets`; this map is
    /// built once in `new()` and never resized at runtime.
    markets: BTreeMap<MarketKey, MarketUnit>,
    /// Static reverse index from each outcome instrument id to the market + leg
    /// it belongs to, built once in `new()`. Routes inbound market-data and
    /// position events to the owning unit; instrument ids are globally unique
    /// across markets (enforced by `validate_config`), so the mapping is total.
    instrument_to_market: BTreeMap<InstrumentId, (MarketKey, Leg)>,

    /// Test-only capture of the order intents the shell would submit live, used
    /// to assert the MarketAction -> intent translation without a runtime.
    #[cfg(test)]
    submit_attempts: Vec<MakerSubmitAttempt>,
}

/// A captured submit attempt (test instrumentation): the market, leg, side,
/// price, and whether the admit gate permitted it.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq)]
struct MakerSubmitAttempt {
    market_key: MarketKey,
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
        // The builder's validate_config already proved the `[selection]` weights
        // construct; re-building here is the single validated-weights source and
        // panics loudly if a build bypassed validation (it cannot on the registry
        // path). The validated max is carried alongside so the per-tick rank reads
        // it without re-checking.
        let selection_weights = config
            .validated_selection_weights()
            .expect("validated binary_oracle_maker selection weights");
        let max_active_markets = config.selection.max_active_markets;
        let spot = MakerSpotFeed::from_config(&config);
        let reference_instrument_id_parsed =
            InstrumentId::from_str(config.reference_instrument_id.as_str()).ok();
        // Build the per-market units + the static instrument->market reverse
        // index from `config.markets` — the single source of truth for the set of
        // markets. Each unit gets its OWN requote budget (per-market throttle) and
        // governor, built from the shared validated knobs. The throttle budget,
        // signed-trade-flow config, and modify capability are shared inputs.
        let flow_config = config.signed_trade_flow_config();
        let mut markets: BTreeMap<MarketKey, MarketUnit> = BTreeMap::new();
        let mut instrument_to_market: BTreeMap<InstrumentId, (MarketKey, Leg)> = BTreeMap::new();
        for market_cfg in &config.markets {
            let market_key = MarketKey::new(market_cfg.market_id.clone());
            let requote_budget = RequoteBudget::new(
                config.requote_max_cost_per_window,
                config.requote_window_ms,
                validated.requote_min_interval_ms(),
            );
            let unit = MarketUnit::from_config(
                market_cfg,
                validated.kill_thresholds(),
                requote_budget,
                config.supports_modify,
                &flow_config,
            );
            instrument_to_market.insert(unit.yes_instrument_id, (market_key.clone(), Leg::Yes));
            instrument_to_market.insert(unit.no_instrument_id, (market_key.clone(), Leg::No));
            markets.insert(market_key, unit);
        }
        Self {
            core: StrategyCore::new(StrategyConfig {
                strategy_id: Some(StrategyId::from(config.strategy_id.as_str())),
                order_id_tag: Some(config.order_id_tag.clone()),
                ..Default::default()
            }),
            family: BinaryFamily,
            spot,
            selection_weights,
            max_active_markets,
            markets,
            instrument_to_market,
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

    /// Borrow the unit for `market_key`. The map is keyed by every configured
    /// market, so a key the shell itself derived (from `instrument_to_market`,
    /// `market_and_leg_for_client_order_id`, or the timer-loop key list) is always
    /// present; `expect` documents that the key came from this strategy's own
    /// config, never from untrusted input.
    fn unit(&self, market_key: &MarketKey) -> &MarketUnit {
        self.markets
            .get(market_key)
            .expect("market_key resolved from this strategy's own config must exist")
    }

    fn unit_mut(&mut self, market_key: &MarketKey) -> &mut MarketUnit {
        self.markets
            .get_mut(market_key)
            .expect("market_key resolved from this strategy's own config must exist")
    }

    /// Resolve the `(market, leg)` an outcome instrument id belongs to via the
    /// static reverse index. Instrument ids are globally unique across markets
    /// (enforced by `validate_config`), so the mapping is total and unambiguous.
    fn market_and_leg_for_instrument(
        &self,
        instrument_id: InstrumentId,
    ) -> Option<(MarketKey, Leg)> {
        self.instrument_to_market
            .get(&instrument_id)
            .map(|(market_key, leg)| (market_key.clone(), *leg))
    }

    /// Resolve the `(market, leg)` a tracked client order id belongs to. COIDs are
    /// market-unique (the id carries the market component), so at most one unit's
    /// slot can match — the scan is unambiguous.
    fn market_and_leg_for_client_order_id(
        &self,
        client_order_id: &ClientOrderId,
    ) -> Option<(MarketKey, Leg)> {
        self.markets.iter().find_map(|(market_key, unit)| {
            [Leg::Yes, Leg::No]
                .into_iter()
                .find(|&leg| unit.slot_tracks(leg, client_order_id))
                .map(|leg| (market_key.clone(), leg))
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

    /// Subscribe every market's outcome legs' L2_MBP books + trades. The maker
    /// quotes both legs of every market from the start, so all books are armed
    /// eagerly in `on_start` (the taker defers its book subscriptions until a
    /// position exists). The per-leg signed-trade-flow buffers are seeded once at
    /// unit construction (`MarketUnit::from_config`), the single source of truth
    /// for a unit's flow state — they are NOT re-seeded here.
    ///
    /// `instrument_to_market` already enumerates every outcome instrument across
    /// every market, so iterating its keys subscribes the full set with no
    /// per-market dispatch.
    fn subscribe_all_legs(&mut self) {
        let instrument_ids: Vec<InstrumentId> = self.instrument_to_market.keys().copied().collect();
        for instrument_id in instrument_ids {
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
            #[cfg(test)]
            let _ = instrument_id;
        }
    }

    fn unsubscribe_all_legs(&mut self) {
        let instrument_ids: Vec<InstrumentId> = self.instrument_to_market.keys().copied().collect();
        for instrument_id in instrument_ids {
            #[cfg(not(test))]
            {
                self.unsubscribe_book_deltas(instrument_id, None, None);
                self.unsubscribe_trades(instrument_id, None, None);
            }
            #[cfg(test)]
            let _ = instrument_id;
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

    /// Compute the book-nudged fair anchor for one market's YES outcome (P(up))
    /// from the shared oracle feed + that market's venue top-of-book. `None` at
    /// any stage = no quote this tick. The strike and maintenance horizon are the
    /// unit's; the spot/RV feed and validated knobs are shared.
    fn anchored_fair(&self, market_key: &MarketKey, now_ms: u64) -> Option<f64> {
        let unit = self.unit(market_key);
        let spot = self.spot.spot()?;
        let realized_vol = self.spot.realized_vol_at(now_ms)?;
        let seconds_to_market_end = self.seconds_to_market_end(market_key, now_ms);
        let inputs = FairProbabilityInputs {
            spot_price: spot,
            strike_price: unit.strike_price,
            seconds_to_market_end,
            realized_vol,
            pricing_kurtosis: self.config.pricing_kurtosis,
        };
        let p_up = bolt_v3_market_families::fair_probability_up_for_family(
            &self.config.family_key,
            &inputs,
        )?;
        // Book nudge: read the YES leg's touch and blend toward the oracle prior.
        let yes_book = unit.book(Leg::Yes);
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

    /// Seconds remaining to one market's resolution, derived from that market's
    /// maintenance horizon when configured. Falls back to the shared reference-tau
    /// horizon so the family model and governor always have a positive `τ` to
    /// read; the τ-floor kill predicate still fires on a too-small value.
    fn seconds_to_market_end(&self, market_key: &MarketKey, now_ms: u64) -> u64 {
        let unit = self.unit(market_key);
        if unit.maintenance_window_duration_ms > 0 && unit.maintenance_window_start_ms > now_ms {
            (unit.maintenance_window_start_ms - now_ms) / MILLIS_PER_SECOND_U64
        } else {
            self.validated.reference_tau() as u64
        }
    }

    /// The governor posture for one market this tick, folding that market's
    /// maintenance gate alongside its W3 market/inventory governor (safety wins —
    /// the more restrictive of the two postures is taken; a kill is never
    /// downgraded back to quoting).
    fn governor_state(
        &self,
        market_key: &MarketKey,
        now_ms: u64,
        oracle_fair: f64,
    ) -> MakerGovernorState {
        let unit = self.unit(market_key);
        let sigma = self.spot.realized_vol_at(now_ms).unwrap_or(f64::NAN);
        let venue_mid = unit.book(Leg::Yes).venue_mid().unwrap_or(f64::NAN);
        let tau = self.seconds_to_market_end(market_key, now_ms) as f64;
        let market_state = unit.governor.resolve(GovernorInputs {
            sigma,
            oracle_fair,
            venue_mid,
            tau,
            net_position: unit.inventory.net_position(),
        });
        // Fold the maintenance gate ONLY when a window is configured. A duration
        // of zero means "no maintenance window configured" (not a degenerate
        // window), so the gate must not veto — consulting it would fail-closed to
        // CancelOnly and silently stop all quoting. A configured window with a
        // genuinely degenerate shape still fails closed inside the gate.
        match self.maintenance_governor_state(market_key, now_ms) {
            Some(maintenance_state) => most_restrictive(market_state, maintenance_state),
            None => market_state,
        }
    }

    /// One market's maintenance-gate posture for this tick, or `None` when that
    /// market configures no maintenance window (`maintenance_window_duration_ms ==
    /// 0`). A configured window resolves through `maintenance_posture` ->
    /// `maintenance_governor_state` (which itself fails closed to CancelOnly on a
    /// degenerate configured shape).
    fn maintenance_governor_state(
        &self,
        market_key: &MarketKey,
        now_ms: u64,
    ) -> Option<MakerGovernorState> {
        let unit = self.unit(market_key);
        if unit.maintenance_window_duration_ms == 0 {
            return None;
        }
        Some(maintenance_governor_state(maintenance_posture(
            now_ms,
            unit.maintenance_window_start_ms,
            unit.maintenance_window_duration_ms,
            unit.maintenance_pre_flatten_lead_ms,
        )))
    }

    /// Build the two desired quote leg prices for one market this tick from the
    /// full maker pipeline. `None` = no quotable target (fail-closed). The
    /// inventory is the unit's; the pricing knobs are shared.
    fn desired_quote_targets(
        &self,
        market_key: &MarketKey,
        oracle_fair: f64,
    ) -> Option<QuoteTargets> {
        let unit = self.unit(market_key);
        let gm = gm_binary_quote(oracle_fair, self.validated.informed_fraction())?;
        let skew = inventory_skew(
            unit.inventory.net_position(),
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

    /// The market-SELECTION pre-pass for this tick: score every configured market
    /// from its LIVE state and return the active set — the top `max_active_markets`
    /// `MarketKey`s by attractiveness. A market produces NO candidate (and is thus
    /// excluded) when its YES leg has no two-sided touch; the pure ranker
    /// additionally drops any degenerate/non-positive-tau candidate (fail-closed).
    /// An empty/all-excluded portfolio yields an empty set — quote nothing.
    ///
    /// Borrow discipline: this reads `&self.markets` and returns an OWNED
    /// `BTreeSet<MarketKey>`, so the caller can hold it across the `&mut self`
    /// per-market loop without an outstanding `&self.markets` borrow.
    fn active_market_set(&self, now_ms: u64) -> std::collections::BTreeSet<MarketKey> {
        let candidates: Vec<MarketCandidate> = self
            .markets
            .iter()
            .filter_map(|(market_key, unit)| self.market_candidate(market_key, unit, now_ms))
            .collect();
        self.selection_weights
            .rank(&candidates)
            .into_iter()
            .take(self.max_active_markets as usize)
            .map(|score| score.market)
            .collect()
    }

    /// Build a [`MarketCandidate`] for one market from its LIVE YES-book touch and
    /// its time-to-resolution, or `None` (excluded) when the YES leg has no
    /// two-sided touch. The captured spread is `best_ask - best_bid`, the
    /// top-of-book liquidity is `min(best_bid_size, best_ask_size)`, both on the
    /// YES book; the time-to-resolution is the unit's `seconds_to_market_end`. A
    /// missing touch on either side is a fail-closed exclusion (the pure ranker
    /// then rejects any candidate it cannot score safely).
    fn market_candidate(
        &self,
        market_key: &MarketKey,
        unit: &MarketUnit,
        now_ms: u64,
    ) -> Option<MarketCandidate> {
        let yes_book = unit.book(Leg::Yes);
        let best_bid = yes_book.best_bid?;
        let best_ask = yes_book.best_ask?;
        let best_bid_size = yes_book.best_bid_size?;
        let best_ask_size = yes_book.best_ask_size?;
        Some(MarketCandidate {
            market: market_key.clone(),
            captured_spread: best_ask - best_bid,
            top_of_book_liquidity: best_bid_size.min(best_ask_size),
            seconds_to_resolution: self.seconds_to_market_end(market_key, now_ms) as f64,
        })
    }

    /// The main quote loop for ONE market — runs on the requote timer (per market)
    /// and on each of that market's book deltas. Every state read/write here is
    /// scoped to the keyed unit; the only shared reads are the oracle fair and the
    /// pricing knobs.
    ///
    /// `active` is the tick's market-selection verdict (see [`active_market_set`]):
    /// a market IN the set quotes normally; a market NOT in the set submits no new
    /// quotes and has its resting quotes canceled (cancel-on-deselect), reusing the
    /// same `CancelAllBothLegs` path the `CancelOnly` governor posture drives.
    fn run_quote_tick(
        &mut self,
        market_key: &MarketKey,
        active: &std::collections::BTreeSet<MarketKey>,
        now_ms: u64,
    ) {
        // Market-selection gate (fail-closed): a deselected market is not quoted,
        // and any resting quote on it is canceled via the SAME per-market cancel
        // path the CancelOnly governor posture uses (drive the lifecycle machine so
        // it stays the single owner of order transitions, then translate to NT).
        if !active.contains(market_key) {
            if let Some(action) = cancel_all_on_kill(
                MakerGovernorState::CancelOnly,
                &mut self.unit_mut(market_key).market,
            ) {
                self.execute_action(market_key, action, None, now_ms);
            }
            return;
        }

        let Some(oracle_fair) = self.anchored_fair(market_key, now_ms) else {
            return;
        };
        let posture = self.governor_state(market_key, now_ms, oracle_fair);

        // Safety postures: drain / reduce-one-side BEFORE any quoting.
        match posture {
            MakerGovernorState::HardFlat(_) | MakerGovernorState::CancelOnly => {
                if let Some(action) =
                    cancel_all_on_kill(posture, &mut self.unit_mut(market_key).market)
                {
                    self.execute_action(market_key, action, None, now_ms);
                }
                return;
            }
            MakerGovernorState::ReduceOnly => {
                // Cancel the inventory-adding side; keep the reducing side. Net
                // long YES => stop adding YES (cancel YES side); net short =>
                // cancel NO side. A flat-but-capped book cancels neither.
                let unit = self.unit_mut(market_key);
                let net = unit.inventory.net_position();
                let action = if net > ZERO_F64 {
                    unit.market.cancel_one_side(Leg::Yes)
                } else if net < ZERO_F64 {
                    unit.market.cancel_one_side(Leg::No)
                } else {
                    None
                };
                if let Some(action) = action {
                    self.execute_action(market_key, action, None, now_ms);
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

        let Some(targets) = self.desired_quote_targets(market_key, oracle_fair) else {
            return;
        };

        let requote_cost = self.requote_cost();
        let requote_threshold = self.config.requote_threshold;
        for leg in [Leg::Yes, Leg::No] {
            let target = Self::target_leg(&targets, leg);
            let unit = self.unit_mut(market_key);
            let resting = unit.slot(leg).resting_price;
            let requote_needed = match resting {
                None => true,
                Some(resting) => (target.price - resting).abs() >= requote_threshold,
            };
            let Some(action) = unit
                .market
                .on_leg_event(leg, LegEvent::QuoteTrigger { requote_needed })
            else {
                continue;
            };
            // Throttle gate: only act on the lifecycle action if the budget allows.
            if !unit.requote_budget.try_acquire(now_ms, requote_cost) {
                continue;
            }
            unit.slot_mut(leg).pending_price = Some(target.price);
            self.execute_action(market_key, action, Some(target), now_ms);
        }
    }

    // -- MarketAction -> NT translation -------------------------------------

    /// Translate one [`MarketAction`] from one market's pure lifecycle/governor
    /// layer into NT order calls. `target` carries the desired leg price when a
    /// Submit/Modify is in play. All live submits route through the admit gate.
    fn execute_action(
        &mut self,
        market_key: &MarketKey,
        action: MarketAction,
        target: Option<QuoteTargetLeg>,
        now_ms: u64,
    ) {
        match action {
            MarketAction::Leg { leg, action } => match action {
                LifecycleAction::Submit | LifecycleAction::Modify => {
                    if let Some(target) = target {
                        self.submit_or_modify_leg(market_key, leg, target, now_ms);
                    }
                }
                LifecycleAction::Cancel => self.cancel_leg(market_key, leg),
            },
            MarketAction::CancelAllBothLegs => {
                self.cancel_all(market_key, None);
                let unit = self.unit_mut(market_key);
                unit.yes_slot = LegSlot::idle();
                unit.no_slot = LegSlot::idle();
            }
            MarketAction::CancelAllOneSide { leg } => {
                // Both binary legs rest BIDS; the cancel scope by instrument side
                // for a resting bid is Buy.
                self.cancel_all_one_side(market_key, leg, OrderSide::Buy);
                *self.unit_mut(market_key).slot_mut(leg) = LegSlot::idle();
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
    fn next_client_order_id(&mut self, market_key: &MarketKey, leg: Leg) -> ClientOrderId {
        #[cfg(not(test))]
        {
            let _ = (market_key, leg);
            self.core.order_factory().generate_client_order_id()
        }
        #[cfg(test)]
        {
            let generation = self.unit(market_key).slot(leg).next_generation;
            // Market component (`market_key.as_str()`) makes the id market-unique,
            // so a slot scan across units is unambiguous: two markets quoting the
            // same leg never collide on a client order id.
            ClientOrderId::from(
                format!(
                    "{}-{}-{:?}-{generation}",
                    self.config.strategy_id,
                    market_key.as_str(),
                    leg
                )
                .as_str(),
            )
        }
    }

    fn submit_or_modify_leg(
        &mut self,
        market_key: &MarketKey,
        leg: Leg,
        target: QuoteTargetLeg,
        now_ms: u64,
    ) {
        let instrument_id = self.unit(market_key).instrument_id_for(leg);
        let order_side = match target.side {
            QuoteSide::Buy => OrderSide::Buy,
            QuoteSide::Sell => OrderSide::Sell,
        };
        let client_order_id = self.next_client_order_id(market_key, leg);
        // Bump the per-leg fence generation BEFORE submit so a fresh order never
        // inherits the stale order's in-flight reports.
        {
            let slot = self.unit_mut(market_key).slot_mut(leg);
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
                    market_key: market_key.clone(),
                    leg,
                    side: target.side,
                    price: target.price,
                    admitted,
                });
                #[cfg(not(test))]
                let _ = admitted;
                self.unit_mut(market_key).slot_mut(leg).last_rest_ms = Some(now_ms);
            }
            Err(error) => {
                #[cfg(test)]
                self.submit_attempts.push(MakerSubmitAttempt {
                    market_key: market_key.clone(),
                    leg,
                    side: target.side,
                    price: target.price,
                    admitted: false,
                });
                log::warn!(
                    "binary_oracle_maker quote submit rejected (gate or build): strategy_id={} market_id={} leg={:?} instrument_id={} error={:#}",
                    self.config.strategy_id,
                    market_key.as_str(),
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

    /// Cancel the single resting order on one market's leg (per-order cancel via
    /// the leg's tracked client order id).
    fn cancel_leg(&mut self, market_key: &MarketKey, leg: Leg) {
        let Some(client_order_id) = self.unit(market_key).slot(leg).client_order_id else {
            return;
        };
        #[cfg(not(test))]
        {
            if let Err(error) = self.cancel_order(client_order_id, Some(self.client_id()), None) {
                log::error!(
                    "binary_oracle_maker cancel_leg failed: strategy_id={} market_id={} leg={:?} error={error}",
                    self.config.strategy_id,
                    market_key.as_str(),
                    leg,
                );
            }
        }
        #[cfg(test)]
        let _ = (market_key, &client_order_id);
    }

    fn cancel_all(&mut self, market_key: &MarketKey, order_side: Option<OrderSide>) {
        #[cfg(not(test))]
        {
            let unit = self.unit(market_key);
            for instrument_id in [unit.yes_instrument_id, unit.no_instrument_id] {
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
        let _ = (market_key, order_side);
    }

    fn cancel_all_one_side(&mut self, market_key: &MarketKey, leg: Leg, order_side: OrderSide) {
        let instrument_id = self.unit(market_key).instrument_id_for(leg);
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
        market_key: &MarketKey,
        leg: Leg,
        client_order_id: &ClientOrderId,
        kind: VenueReportKind,
        now_ms: u64,
    ) -> Option<MarketAction> {
        let generation = self
            .unit(market_key)
            .slot(leg)
            .fence
            .expected()
            .map(OrderIdentity::generation)
            .unwrap_or(INITIAL_GENERATION);
        let report = VenueReport {
            client_order_id: FenceClientOrderId::new(client_order_id.to_string()),
            generation,
            kind,
        };
        match self.unit(market_key).slot(leg).fence.admit(&report) {
            Ok(event) => {
                self.apply_leg_state_side_effects(market_key, leg, event, now_ms);
                self.unit_mut(market_key).market.on_leg_event(leg, event)
            }
            Err(reject) => {
                self.log_fence_reject(market_key, leg, client_order_id, &reject);
                None
            }
        }
    }

    fn log_fence_reject(
        &self,
        market_key: &MarketKey,
        leg: Leg,
        client_order_id: &ClientOrderId,
        reject: &FenceReject,
    ) {
        log::warn!(
            "binary_oracle_maker fence dropped a venue report (fail-closed): strategy_id={} market_id={} leg={:?} client_order_id={} reject={:?}",
            self.config.strategy_id,
            market_key.as_str(),
            leg,
            client_order_id,
            reject,
        );
    }

    /// Update one market's shell-owned slot state for an admitted leg event before
    /// the lifecycle machine consumes it (resting price, last-rest timestamp,
    /// fence clear on terminal events).
    fn apply_leg_state_side_effects(
        &mut self,
        market_key: &MarketKey,
        leg: Leg,
        event: LegEvent,
        now_ms: u64,
    ) {
        let slot = self.unit_mut(market_key).slot_mut(leg);
        match event {
            LegEvent::Accepted | LegEvent::Modified => {
                slot.resting_price = slot.pending_price.take().or(slot.resting_price);
                slot.last_rest_ms = Some(now_ms);
            }
            LegEvent::Filled | LegEvent::Rejected => {
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
                slot.resting_price = None;
                slot.last_rest_ms = None;
            }
            LegEvent::CancelRejected | LegEvent::ModifyRejected | LegEvent::QuoteTrigger { .. } => {
            }
        }
    }

    /// Apply a confirmed fill to one market's inventory book. A binary maker rests
    /// bids on both legs, so a maker fill is a Buy on that leg.
    fn apply_fill_to_inventory(
        &mut self,
        market_key: &MarketKey,
        leg: Leg,
        order_side: OrderSide,
        qty: f64,
    ) {
        let side = match order_side {
            OrderSide::Buy => QuoteSide::Buy,
            OrderSide::Sell => QuoteSide::Sell,
            _ => return,
        };
        let _ = self
            .unit_mut(market_key)
            .inventory
            .apply_fill(leg, side, qty);
    }

    // -- ops cadence: stale-quote alarm + maintenance -----------------------

    fn run_ops_tick(&mut self, market_key: &MarketKey, now_ms: u64) {
        let unit = self.unit(market_key);
        let alarm = MarketStaleQuoteAlarm::evaluate(
            now_ms,
            unit.yes_slot.last_rest_ms,
            unit.no_slot.last_rest_ms,
            self.config.max_rest_age_ms,
        );
        if alarm.any_stale() {
            log::info!(
                "binary_oracle_maker stale-quote alarm: strategy_id={} market_id={} stale_legs={}",
                self.config.strategy_id,
                market_key.as_str(),
                alarm.stale_legs().len(),
            );
            for action in alarm.refresh_plan() {
                // Drive the cancel through the lifecycle machine so it remains the
                // single owner of order transitions, then translate to NT.
                if let MarketAction::Leg { leg, .. } = action
                    && let Some(lifecycle_action) = self.unit_mut(market_key).market.cancel_leg(leg)
                {
                    self.execute_action(market_key, lifecycle_action, None, now_ms);
                }
            }
        }
        // Maintenance gate is also consulted in the quote tick's governor fold;
        // the ops cadence additionally drains both legs inside / approaching a
        // configured window. With no window configured the gate is skipped (it
        // does not veto), matching `governor_state`.
        if let Some(maintenance_state) = self.maintenance_governor_state(market_key, now_ms)
            && let Some(action) =
                cancel_all_on_kill(maintenance_state, &mut self.unit_mut(market_key).market)
        {
            self.execute_action(market_key, action, None, now_ms);
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
    fn run_reconnect_watchdog(&mut self, market_key: &MarketKey, now_ms: u64) {
        let gap_threshold = self.config.data_gap_staleness_ms;
        if gap_threshold == 0 {
            return;
        }
        let unit = self.unit(market_key);
        let yes_gapped = data_gap_exceeds(unit.yes_last_data_ms, now_ms, gap_threshold);
        let no_gapped = data_gap_exceeds(unit.no_last_data_ms, now_ms, gap_threshold);
        if !(yes_gapped || no_gapped) {
            return;
        }
        log::warn!(
            "binary_oracle_maker data-gap watchdog tripped (probable reconnect): strategy_id={} market_id={} yes_gapped={} no_gapped={}",
            self.config.strategy_id,
            market_key.as_str(),
            yes_gapped,
            no_gapped,
        );
        // Build the per-leg reconcile snapshots. `believed_resting` is the shell's
        // local belief; `venue_reports_open` is venue truth read from the NT cache
        // (open orders the exec client/adapter re-synced on reconnect).
        let yes_snapshot = LegReconcileSnapshot::new(
            Leg::Yes,
            self.unit(market_key).slot(Leg::Yes).resting_price.is_some(),
            self.venue_reports_open(market_key, Leg::Yes),
        );
        let no_snapshot = LegReconcileSnapshot::new(
            Leg::No,
            self.unit(market_key).slot(Leg::No).resting_price.is_some(),
            self.venue_reports_open(market_key, Leg::No),
        );
        let Some(snapshot) = MarketReconcileSnapshot::new(yes_snapshot, no_snapshot) else {
            return;
        };
        for action in snapshot.reconcile() {
            let target = match action {
                MarketAction::Leg {
                    leg,
                    action: LifecycleAction::Submit,
                } => self.pending_target_for_resubmit(market_key, leg),
                _ => None,
            };
            self.execute_action(market_key, action, target, now_ms);
        }
        // Reset the gap clocks so a single trip does not re-fire every tick until
        // fresh data arrives.
        let unit = self.unit_mut(market_key);
        if yes_gapped {
            unit.yes_last_data_ms = Some(now_ms);
        }
        if no_gapped {
            unit.no_last_data_ms = Some(now_ms);
        }
    }

    /// Whether the venue reports an open order on one market's `leg` after a
    /// reconnect, read from the NT cache by the leg's tracked client order id. In
    /// tests (no registered cache) this is conservatively `false`.
    fn venue_reports_open(&self, market_key: &MarketKey, leg: Leg) -> bool {
        #[cfg(not(test))]
        {
            if let Some(client_order_id) = self.unit(market_key).slot(leg).client_order_id.as_ref()
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
            let _ = (market_key, leg);
            false
        }
    }

    // -- settlement on resolution -------------------------------------------

    /// Settle the held YES/NO lots at a binary resolution. The shell reads NT's
    /// per-instrument position + `avg_px_open` for both outcome instruments,
    /// builds two [`TokenLot`]s from the SAME market, and calls [`settle`]. On the
    /// live path NT does NOT auto-book the 0/1 close (NEEDS-VERIFY Q1 resolved),
    /// so this is the sole source of the settlement payout.
    fn settle_on_resolution(&mut self, market_key: &MarketKey, outcome: SettlementOutcome) {
        let (yes_instrument_id, no_instrument_id) = {
            let unit = self.unit(market_key);
            (unit.yes_instrument_id, unit.no_instrument_id)
        };
        let yes_lot = self.token_lot_for_instrument(yes_instrument_id);
        let no_lot = self.token_lot_for_instrument(no_instrument_id);
        let (Some(yes_lot), Some(no_lot)) = (yes_lot, no_lot) else {
            return;
        };
        let Some(booking) = settle(yes_lot, no_lot, outcome) else {
            log::error!(
                "binary_oracle_maker settlement booking failed (degenerate lots): strategy_id={} market_id={}",
                self.config.strategy_id,
                market_key.as_str(),
            );
            return;
        };
        log::info!(
            "binary_oracle_maker settled binary resolution: strategy_id={} market_id={} outcome={:?} payout={} realized_pnl={}",
            self.config.strategy_id,
            market_key.as_str(),
            outcome,
            booking.payout(),
            booking.realized_pnl(),
        );
        let unit = self.unit_mut(market_key);
        unit.last_settlement_pnl = Some(booking.realized_pnl());
        // Resolution flattens this market's maker: reset inventory.
        unit.inventory = MakerInventory::flat();
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

    /// Resolve the `(market, settled outcome)` for a closed position by instrument
    /// identity — NT owns the position; the shell only names the market + side
    /// that closed. `SettlementOutcome` is the closing `Leg`. `None` if the
    /// instrument is not one of this maker's legs.
    fn settlement_outcome_for_close(
        &self,
        instrument_id: InstrumentId,
    ) -> Option<(MarketKey, SettlementOutcome)> {
        self.market_and_leg_for_instrument(instrument_id)
    }

    /// Touch one market's per-leg data-gap watchdog timestamp from a market-data
    /// event the handler already routed to `(market_key, leg)`.
    fn touch_leg_data(&mut self, market_key: &MarketKey, leg: Leg, now_ms: u64) {
        let unit = self.unit_mut(market_key);
        match leg {
            Leg::Yes => unit.yes_last_data_ms = Some(now_ms),
            Leg::No => unit.no_last_data_ms = Some(now_ms),
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

    /// The pending quote target for one market's requote-cancel resubmit,
    /// reconstructed from the slot's pending price (the desired price the cancel
    /// was emitted for).
    fn pending_target_for_resubmit(
        &self,
        market_key: &MarketKey,
        leg: Leg,
    ) -> Option<QuoteTargetLeg> {
        self.unit(market_key)
            .slot(leg)
            .pending_price
            .map(|price| QuoteTargetLeg {
                // Both binary legs rest bids.
                side: QuoteSide::Buy,
                price,
            })
    }

    /// Adopt NT's authoritative position into the owning market's maker inventory
    /// model (NT owns PnL/position). The position event's instrument id is routed
    /// to its market + leg via `instrument_to_market`; an instrument that belongs
    /// to no market is ignored (fail-closed).
    fn adopt_position_into_inventory(
        &mut self,
        instrument_id: InstrumentId,
        side: PositionSide,
        quantity: f64,
    ) {
        let Some((market_key, leg)) = self
            .instrument_to_market
            .get(&instrument_id)
            .map(|(market_key, leg)| (market_key.clone(), *leg))
        else {
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
        let _ = self
            .unit_mut(&market_key)
            .inventory
            .apply_fill(leg, quote_side, quantity);
    }
}

// ---------------------------------------------------------------------------
// NT lifecycle / data hooks (DataActor block — &event -> Result<()>)
// ---------------------------------------------------------------------------

impl DataActor for BinaryOracleMaker {
    fn on_start(&mut self) -> Result<()> {
        // Subscribe the shared reference once, then arm every market's legs. The
        // per-leg trade-flow buffers were seeded at unit construction. A single
        // requote timer + single ops timer drive ALL markets on a shared cadence
        // (no per-market timers).
        self.subscribe_reference_quotes();
        self.subscribe_all_legs();
        self.register_timers();
        Ok(())
    }

    fn on_stop(&mut self) -> Result<()> {
        // Cancel resting quotes on every market's legs (a stop must not leak live
        // quotes), unsubscribe, deregister timers.
        let keys: Vec<MarketKey> = self.markets.keys().cloned().collect();
        for key in keys {
            self.cancel_all(&key, None);
        }
        self.unsubscribe_reference_quotes();
        self.unsubscribe_all_legs();
        self.deregister_timers();
        Ok(())
    }

    fn on_time_event(&mut self, event: &TimeEvent) -> Result<()> {
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        let name = event.name.as_str();
        // Collect the market keys FIRST so the per-market `&mut self` calls below
        // do not hold a borrow of `self.markets` across the loop body.
        let keys: Vec<MarketKey> = self.markets.keys().cloned().collect();
        if name == self.requote_timer_name() {
            // Market-selection pre-pass, computed ONCE per tick into an owned set
            // before the `&mut self` per-market loop (same collect-keys-first
            // borrow pattern). Markets outside the active set are not quoted and
            // have their resting quotes canceled inside `run_quote_tick`.
            let active = self.active_market_set(now_ms);
            for key in &keys {
                // Reconnect watchdog runs first so a probable reconnect reconciles
                // before any fresh quoting this tick.
                self.run_reconnect_watchdog(key, now_ms);
                self.run_quote_tick(key, &active, now_ms);
            }
        } else if name == self.ops_timer_name() {
            for key in &keys {
                self.run_ops_tick(key, now_ms);
            }
        }
        Ok(())
    }

    fn on_quote(&mut self, quote: &QuoteTick) -> Result<()> {
        // Drive the SHARED maker spot / realized-vol feed from the reference
        // instrument. The reference oracle is shared across all markets.
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
        let Some((market_key, leg)) = self.market_and_leg_for_instrument(deltas.instrument_id)
        else {
            return Ok(());
        };
        let now_ms = self.clock().timestamp_ns().as_u64() / NANOS_PER_MILLI_U64;
        {
            let unit = self.unit_mut(&market_key);
            match leg {
                Leg::Yes => unit.yes_book.update_from_deltas(deltas),
                Leg::No => unit.no_book.update_from_deltas(deltas),
            }
        }
        self.touch_leg_data(&market_key, leg, now_ms);
        // A book move is a reprice trigger for THIS market alongside the requote
        // timer. Selection depends on live book state, so re-score the portfolio
        // (the book that just moved may now rank in or out of the active set);
        // gate this market's reprice on the fresh verdict — single selection path.
        let active = self.active_market_set(now_ms);
        self.run_quote_tick(&market_key, &active, now_ms);
        Ok(())
    }

    fn on_trade(&mut self, trade: &TradeTick) -> Result<()> {
        let Some((market_key, leg)) = self.market_and_leg_for_instrument(trade.instrument_id)
        else {
            return Ok(());
        };
        let now_ms = trade.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        self.unit_mut(&market_key)
            .trade_flow_mut(leg)
            .observe(trade);
        self.touch_leg_data(&market_key, leg, now_ms);
        Ok(())
    }

    fn on_order_filled(&mut self, event: &OrderFilled) -> Result<()> {
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        // Resolve the market + leg from the (market-unique) client order id,
        // falling back to the instrument-id index when the order is not tracked.
        let Some((market_key, leg)) = self
            .market_and_leg_for_client_order_id(&event.client_order_id)
            .or_else(|| self.market_and_leg_for_instrument(event.instrument_id))
        else {
            return Ok(());
        };
        // A partial fill (remainder still working) is not a lifecycle Filled
        // event; only a fill that leaves zero quantity working is. NT's
        // OrderFilled carries cumulative state — gate on the order being closed in
        // the cache (a fully-filled order is closed); treat an order no longer in
        // the cache as a full fill.
        let fully_filled = self.order_is_closed(&event.client_order_id);
        self.apply_fill_to_inventory(&market_key, leg, event.order_side, event.last_qty.as_f64());
        if fully_filled
            && let Some(action) = self.ingest_venue_report(
                &market_key,
                leg,
                &event.client_order_id,
                VenueReportKind::Filled,
                now_ms,
            )
        {
            self.execute_action(&market_key, action, None, now_ms);
        }
        Ok(())
    }

    fn on_order_canceled(&mut self, event: &OrderCanceled) -> Result<()> {
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        if let Some((market_key, leg)) =
            self.market_and_leg_for_client_order_id(&event.client_order_id)
            && let Some(action) = self.ingest_venue_report(
                &market_key,
                leg,
                &event.client_order_id,
                VenueReportKind::Canceled,
                now_ms,
            )
        {
            // A confirmed requote cancel emits the replacement Submit; drive the
            // resubmit through the slot's pending price when present.
            let target = self.pending_target_for_resubmit(&market_key, leg);
            self.execute_action(&market_key, action, target, now_ms);
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
        if let Some((market_key, leg)) =
            self.market_and_leg_for_client_order_id(&event.client_order_id)
            && let Some(action) = self.ingest_venue_report(
                &market_key,
                leg,
                &event.client_order_id,
                VenueReportKind::Rejected,
                now_ms,
            )
        {
            self.execute_action(&market_key, action, None, now_ms);
        }
    }

    fn on_order_expired(&mut self, event: nautilus_model::events::OrderExpired) {
        // A GTD/short-TTL quote expiry frees the slot; treat it as a cancel (the
        // order left the book without a fill) so the requote loop replaces it.
        let now_ms = event.ts_event.as_u64() / NANOS_PER_MILLI_U64;
        if let Some((market_key, leg)) =
            self.market_and_leg_for_client_order_id(&event.client_order_id)
            && let Some(action) = self.ingest_venue_report(
                &market_key,
                leg,
                &event.client_order_id,
                VenueReportKind::Canceled,
                now_ms,
            )
        {
            self.execute_action(&market_key, action, None, now_ms);
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
        // close. Settle the held YES/NO lots for the owning market here (NT does
        // not auto-book the live 0/1 payout). See module docs for the seam.
        if let Some((market_key, outcome)) = self.settlement_outcome_for_close(event.instrument_id)
        {
            self.settle_on_resolution(&market_key, outcome);
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
        // Fail-loud: surface EVERY [[markets]] error (non-empty, unique ids,
        // parseable + globally-unique instrument ids), collect-all.
        let markets_context = format!("{field_prefix}.markets");
        for message in config.validate_markets(&markets_context) {
            errors.push(ValidationError {
                field: format!("{field_prefix}.markets"),
                code: MARKETS_INVALID_CODE,
                message,
            });
        }
        // Fail-loud: surface EVERY [selection] error (degenerate weights,
        // sub-unit max_active_markets), collect-all.
        let selection_context = format!("{field_prefix}.selection");
        for message in config.validate_selection(&selection_context) {
            errors.push(ValidationError {
                field: format!("{field_prefix}.selection"),
                code: SELECTION_INVALID_CODE,
                message,
            });
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

    /// The single market's stable key for the one-market fixture. A
    /// single-market deployment is simply a `[[markets]]` list of length one.
    fn single_market_key() -> MarketKey {
        MarketKey::new("MKT-A".to_string())
    }

    /// A full, valid SINGLE-market maker config — a `[[markets]]` list of length
    /// one (proving single-market = list-of-1, no dual path). Literals here are
    /// test-only (the runtime-literal verifier strips this `#[cfg(test)] mod
    /// tests` block).
    fn valid_config_toml() -> &'static str {
        r#"
strategy_id = "MAKER-001"
order_id_tag = "001"
client_id = "POLYMARKET"
family_key = "updown"
reference_instrument_id = "BTCUSDT-PERP.BINANCE"
reference_venue = "BINANCE"
quote_size = 5.0
supports_modify = false
requote_cadence_seconds = 1
ops_cadence_seconds = 5
requote_max_cost_per_window = 100
requote_window_ms = 60000
max_rest_age_ms = 30000
data_gap_staleness_ms = 10000
requote_threshold = 0.001
pricing_kurtosis = 0.0
vol_window_secs = 600
vol_gap_reset_secs = 120
vol_min_observations = 3
vol_bridge_valid_secs = 600
trade_flow_window_secs = 300
trade_flow_max_samples = 256

[[markets]]
market_id = "MKT-A"
yes_instrument_id = "YESTOKEN.POLYMARKET"
no_instrument_id = "NOTOKEN.POLYMARKET"
strike_price = 100.0
maintenance_window_start_ms = 0
maintenance_window_duration_ms = 0
maintenance_pre_flatten_lead_ms = 0

[selection]
spread_weight = 1.0
liquidity_weight = 1.0
tau_weight = 0.0
max_active_markets = 8

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
        warm_spot_feed_for(maker, &single_market_key())
    }

    /// As [`warm_spot_feed`] but seeds a specific market's YES-leg touch — used by
    /// the multi-market isolation test to prove per-market state independence.
    fn warm_spot_feed_for(maker: &mut BinaryOracleMaker, market_key: &MarketKey) -> u64 {
        let base = 100.0_f64;
        let mut ts = 1_000_u64;
        for i in 0..8u64 {
            let price = base + if i % 2 == 0 { 0.5 } else { -0.5 };
            maker.spot.observe(price, ts);
            ts += 1_000;
        }
        // Seed a YES-leg touch straddling ~0.5 (the spot==strike fair) so the
        // governor's |oracle_fair - venue_mid| basis stays well inside basis_cap.
        let unit = maker.unit_mut(market_key);
        unit.yes_book.best_bid = Some(0.49);
        unit.yes_book.best_bid_size = Some(10.0);
        unit.yes_book.best_ask = Some(0.51);
        unit.yes_book.best_ask_size = Some(10.0);
        ts
    }

    /// Run the per-market quote tick through the production selection pre-pass:
    /// compute the active set exactly as `on_time_event` does, then drive one
    /// market's tick against it. The single test entry to `run_quote_tick` so
    /// every quote-loop test exercises the real selection gate.
    fn run_quote_tick_with_selection(
        maker: &mut BinaryOracleMaker,
        market_key: &MarketKey,
        now_ms: u64,
    ) {
        let active = maker.active_market_set(now_ms);
        maker.run_quote_tick(market_key, &active, now_ms);
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
        let key = single_market_key();
        let now_ms = warm_spot_feed(&mut maker);

        let fair = maker
            .anchored_fair(&key, now_ms)
            .expect("warm spot/RV must yield an oracle fair");
        assert!(maker.governor_state(&key, now_ms, fair) == MakerGovernorState::Quoting);
        let targets = maker
            .desired_quote_targets(&key, fair)
            .expect("a healthy tick must produce quote targets");
        assert!(targets.leg_a.price > 0.0 && targets.leg_a.price < 1.0);
        assert!(targets.leg_b.price > 0.0 && targets.leg_b.price < 1.0);

        run_quote_tick_with_selection(&mut maker, &key, now_ms);
        assert_eq!(
            maker.submit_attempts.len(),
            2,
            "an idle-both-legs quoting tick must translate into two leg submits"
        );
        let market = &maker.unit(&key).market;
        assert_eq!(market.leg_state(Leg::Yes), LegState::SubmitPending);
        assert_eq!(market.leg_state(Leg::No), LegState::SubmitPending);
    }

    #[test]
    fn no_oracle_fair_skips_quoting_fail_closed() {
        let mut maker = make_maker(unarmed_admission());
        let key = single_market_key();
        run_quote_tick_with_selection(&mut maker, &key, 1_000);
        assert!(
            maker.submit_attempts.is_empty(),
            "a tick with no oracle fair must emit no quotes (fail-closed)"
        );
        assert_eq!(maker.unit(&key).market.leg_state(Leg::Yes), LegState::Idle);
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
        let key = single_market_key();
        let now_ms = warm_spot_feed(&mut maker);
        run_quote_tick_with_selection(&mut maker, &key, now_ms);
        let client_order_id = maker
            .unit(&key)
            .slot(Leg::Yes)
            .client_order_id
            .expect("a YES submit attempt must have stamped a client order id");
        assert_eq!(
            maker.unit(&key).market.leg_state(Leg::Yes),
            LegState::SubmitPending
        );

        // Forge a STALE report: right client id, older generation than expected.
        let stale_identity =
            OrderIdentity::new(FenceClientOrderId::new(client_order_id.to_string()), 5);
        maker
            .unit_mut(&key)
            .yes_slot
            .fence
            .requote_to(stale_identity);
        let stale_report = VenueReport {
            client_order_id: FenceClientOrderId::new(client_order_id.to_string()),
            generation: 1, // < 5 expected => StaleGeneration
            kind: VenueReportKind::Filled,
        };
        assert!(
            maker
                .unit(&key)
                .yes_slot
                .fence
                .admit(&stale_report)
                .is_err(),
            "a stale-generation report must be rejected"
        );

        // And through the shell's ingest path: a genuinely foreign client id is
        // dropped (fail-closed) and produces no MarketAction.
        let foreign = maker.ingest_venue_report(
            &key,
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
        let key = single_market_key();
        let now_ms = warm_spot_feed(&mut maker);
        run_quote_tick_with_selection(&mut maker, &key, now_ms);
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
        let key = single_market_key();
        maker.apply_fill_to_inventory(&key, Leg::Yes, OrderSide::Buy, 3.0);
        assert!((maker.unit(&key).inventory.net_position() - 3.0).abs() < 1e-9);
        maker.apply_fill_to_inventory(&key, Leg::No, OrderSide::Buy, 1.0);
        assert!((maker.unit(&key).inventory.net_position() - 2.0).abs() < 1e-9);
    }

    // 7. FR-041 multi-market state isolation ---------------------------------

    /// A full, valid TWO-market maker config. Shared knobs stay top-level; each
    /// market owns its own outcome instruments + strike + maintenance window.
    fn two_market_config_toml() -> &'static str {
        r#"
strategy_id = "MAKER-001"
order_id_tag = "001"
client_id = "POLYMARKET"
family_key = "updown"
reference_instrument_id = "BTCUSDT-PERP.BINANCE"
reference_venue = "BINANCE"
quote_size = 5.0
supports_modify = false
requote_cadence_seconds = 1
ops_cadence_seconds = 5
requote_max_cost_per_window = 100
requote_window_ms = 60000
max_rest_age_ms = 30000
data_gap_staleness_ms = 10000
requote_threshold = 0.001
pricing_kurtosis = 0.0
vol_window_secs = 600
vol_gap_reset_secs = 120
vol_min_observations = 3
vol_bridge_valid_secs = 600
trade_flow_window_secs = 300
trade_flow_max_samples = 256

[[markets]]
market_id = "MKT-A"
yes_instrument_id = "YESA.POLYMARKET"
no_instrument_id = "NOA.POLYMARKET"
strike_price = 100.0
maintenance_window_start_ms = 0
maintenance_window_duration_ms = 0
maintenance_pre_flatten_lead_ms = 0

[[markets]]
market_id = "MKT-B"
yes_instrument_id = "YESB.POLYMARKET"
no_instrument_id = "NOB.POLYMARKET"
strike_price = 200.0
maintenance_window_start_ms = 0
maintenance_window_duration_ms = 0
maintenance_pre_flatten_lead_ms = 0

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

[selection]
spread_weight = 1.0
liquidity_weight = 1.0
tau_weight = 0.0
max_active_markets = 8
"#
    }

    fn make_two_market_maker(admission: Arc<BoltV3SubmitAdmissionState>) -> BinaryOracleMaker {
        let raw = toml::from_str::<Value>(two_market_config_toml())
            .expect("two-market config must parse");
        // It must also clear the builder's market validation (non-empty, unique
        // ids, parseable + globally-unique instruments).
        let mut errors: Vec<ValidationError> = Vec::new();
        BinaryOracleMakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
        assert!(
            errors.is_empty(),
            "a valid two-market config must pass validation, got: {errors:?}"
        );
        let config =
            BinaryOracleMakerBuilder::parse_config(&raw).expect("two-market config deserializes");
        BinaryOracleMaker::new(config, test_context_with(admission))
    }

    #[test]
    fn multi_market_state_is_isolated() {
        let mut maker = make_two_market_maker(unarmed_admission());
        let key_a = MarketKey::new("MKT-A".to_string());
        let key_b = MarketKey::new("MKT-B".to_string());

        // (a) The same leg in two markets yields DISTINCT client order ids — the
        // market component is present, so a slot scan can never be ambiguous.
        let coid_a = maker.next_client_order_id(&key_a, Leg::Yes);
        let coid_b = maker.next_client_order_id(&key_b, Leg::Yes);
        assert_ne!(
            coid_a, coid_b,
            "the same leg across two markets must produce distinct client order ids"
        );
        assert!(
            coid_a.to_string().contains("MKT-A") && coid_b.to_string().contains("MKT-B"),
            "each client order id must carry its market component: {coid_a} / {coid_b}"
        );

        // (b) instrument_to_market routes each of the 4 instrument ids to the
        // correct (market, leg).
        let yes_a = InstrumentId::from("YESA.POLYMARKET");
        let no_a = InstrumentId::from("NOA.POLYMARKET");
        let yes_b = InstrumentId::from("YESB.POLYMARKET");
        let no_b = InstrumentId::from("NOB.POLYMARKET");
        assert_eq!(
            maker.market_and_leg_for_instrument(yes_a),
            Some((key_a.clone(), Leg::Yes))
        );
        assert_eq!(
            maker.market_and_leg_for_instrument(no_a),
            Some((key_a.clone(), Leg::No))
        );
        assert_eq!(
            maker.market_and_leg_for_instrument(yes_b),
            Some((key_b.clone(), Leg::Yes))
        );
        assert_eq!(
            maker.market_and_leg_for_instrument(no_b),
            Some((key_b.clone(), Leg::No))
        );

        // (c) Mutating market A does NOT touch market B's inventory or books.
        maker.apply_fill_to_inventory(&key_a, Leg::Yes, OrderSide::Buy, 7.0);
        maker.unit_mut(&key_a).yes_book.best_bid = Some(0.42);
        assert!(
            (maker.unit(&key_a).inventory.net_position() - 7.0).abs() < 1e-9,
            "market A inventory must reflect its own fill"
        );
        assert!(
            (maker.unit(&key_b).inventory.net_position()).abs() < 1e-9,
            "market B inventory must remain flat — A's fill must not leak"
        );
        assert_eq!(
            maker.unit(&key_b).yes_book.best_bid,
            None,
            "market B book must remain untouched by A's book mutation"
        );
        assert_eq!(
            maker.unit(&key_a).yes_book.best_bid,
            Some(0.42),
            "market A book must reflect its own mutation"
        );
    }

    /// The config head before the first `[[markets]]` table and the
    /// `[parameters]` table (with its leading marker), split from the canonical
    /// two-market fixture so a test can splice a different markets shape between
    /// them without restating the shared knobs.
    fn config_head_and_parameters() -> (&'static str, String) {
        let head = two_market_config_toml()
            .split("[[markets]]")
            .next()
            .expect("config head before markets");
        let params_tail = two_market_config_toml()
            .split_once("[parameters]")
            .expect("config has a [parameters] table")
            .1;
        (head, format!("[parameters]{params_tail}"))
    }

    #[test]
    fn empty_markets_list_fails_validation() {
        // An EMPTY `markets = []` list must be rejected loud (fail-closed): there
        // is no implicit single market. An explicit empty inline array
        // deserializes fine, so the emptiness is caught by validate_markets (not
        // by a missing-field error).
        let (head, parameters) = config_head_and_parameters();
        let empty_markets = format!("{head}markets = []\n\n{parameters}");
        let raw =
            toml::from_str::<Value>(&empty_markets).expect("config with empty markets parses");
        let mut errors: Vec<ValidationError> = Vec::new();
        BinaryOracleMakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
        assert!(
            errors.iter().any(|e| e.message.contains("at least one")),
            "an empty markets list must fail loud, got: {errors:?}"
        );
    }

    #[test]
    fn an_absent_markets_field_is_a_loud_deserialize_error() {
        // A config that omits `markets` entirely must fail to deserialize (the
        // field is required — there is no defaulted single market).
        let (head, parameters) = config_head_and_parameters();
        let no_markets = format!("{head}{parameters}");
        let raw =
            toml::from_str::<Value>(&no_markets).expect("config without markets still parses");
        let result = BinaryOracleMakerBuilder::parse_config(&raw);
        assert!(
            result.is_err(),
            "a config missing the required markets field must fail to deserialize"
        );
    }

    #[test]
    fn duplicate_instrument_across_markets_fails_validation() {
        // Reusing market A's YES instrument as market B's YES instrument must be
        // rejected — outcome instruments are globally unique.
        let toml = two_market_config_toml().replace(
            "yes_instrument_id = \"YESB.POLYMARKET\"",
            "yes_instrument_id = \"YESA.POLYMARKET\"",
        );
        let raw = toml::from_str::<Value>(&toml).expect("config still parses");
        let mut errors: Vec<ValidationError> = Vec::new();
        BinaryOracleMakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("more than one market")),
            "a duplicate cross-market instrument must fail loud, got: {errors:?}"
        );
    }

    #[test]
    fn duplicate_market_id_fails_validation() {
        let toml =
            two_market_config_toml().replace("market_id = \"MKT-B\"", "market_id = \"MKT-A\"");
        let raw = toml::from_str::<Value>(&toml).expect("config still parses");
        let mut errors: Vec<ValidationError> = Vec::new();
        BinaryOracleMakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("duplicate market_id")),
            "a duplicate market_id must fail loud, got: {errors:?}"
        );
    }

    // 8. FR-041 market selection (rank + top-K + fail-closed exclude + cancel
    //    on deselect) -------------------------------------------------------

    /// A full, valid THREE-market maker config. Spread-only selection
    /// (`spread_weight = 1`, others `0`) with `max_active_markets = 1` so the
    /// pre-pass trims to a single top-K market — the controllable shape for the
    /// rank + top-K + exclusion assertions. Shared knobs/oracle stay top-level;
    /// each market owns its own outcome instruments.
    fn three_market_config_toml() -> &'static str {
        r#"
strategy_id = "MAKER-001"
order_id_tag = "001"
client_id = "POLYMARKET"
family_key = "updown"
reference_instrument_id = "BTCUSDT-PERP.BINANCE"
reference_venue = "BINANCE"
quote_size = 5.0
supports_modify = false
requote_cadence_seconds = 1
ops_cadence_seconds = 5
requote_max_cost_per_window = 100
requote_window_ms = 60000
max_rest_age_ms = 30000
data_gap_staleness_ms = 10000
requote_threshold = 0.001
pricing_kurtosis = 0.0
vol_window_secs = 600
vol_gap_reset_secs = 120
vol_min_observations = 3
vol_bridge_valid_secs = 600
trade_flow_window_secs = 300
trade_flow_max_samples = 256

[[markets]]
market_id = "MKT-A"
yes_instrument_id = "YESA.POLYMARKET"
no_instrument_id = "NOA.POLYMARKET"
strike_price = 100.0
maintenance_window_start_ms = 0
maintenance_window_duration_ms = 0
maintenance_pre_flatten_lead_ms = 0

[[markets]]
market_id = "MKT-B"
yes_instrument_id = "YESB.POLYMARKET"
no_instrument_id = "NOB.POLYMARKET"
strike_price = 200.0
maintenance_window_start_ms = 0
maintenance_window_duration_ms = 0
maintenance_pre_flatten_lead_ms = 0

[[markets]]
market_id = "MKT-C"
yes_instrument_id = "YESC.POLYMARKET"
no_instrument_id = "NOC.POLYMARKET"
strike_price = 300.0
maintenance_window_start_ms = 0
maintenance_window_duration_ms = 0
maintenance_pre_flatten_lead_ms = 0

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

[selection]
spread_weight = 1.0
liquidity_weight = 0.0
tau_weight = 0.0
max_active_markets = 1
"#
    }

    fn make_three_market_maker(admission: Arc<BoltV3SubmitAdmissionState>) -> BinaryOracleMaker {
        let raw = toml::from_str::<Value>(three_market_config_toml())
            .expect("three-market config must parse");
        let mut errors: Vec<ValidationError> = Vec::new();
        BinaryOracleMakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
        assert!(
            errors.is_empty(),
            "a valid three-market config must pass validation, got: {errors:?}"
        );
        let config =
            BinaryOracleMakerBuilder::parse_config(&raw).expect("three-market config deserializes");
        BinaryOracleMaker::new(config, test_context_with(admission))
    }

    /// Seed a YES-leg two-sided touch for one market with an explicit captured
    /// spread (`best_ask - best_bid`) and symmetric size — the live state the
    /// selection pre-pass reads to build a candidate.
    fn seed_yes_touch(
        maker: &mut BinaryOracleMaker,
        market_key: &MarketKey,
        bid: f64,
        ask: f64,
        size: f64,
    ) {
        let unit = maker.unit_mut(market_key);
        unit.yes_book.best_bid = Some(bid);
        unit.yes_book.best_ask = Some(ask);
        unit.yes_book.best_bid_size = Some(size);
        unit.yes_book.best_ask_size = Some(size);
    }

    #[test]
    fn selection_quotes_top_k_and_excludes_degenerate() {
        let mut maker = make_three_market_maker(unarmed_admission());
        let key_a = MarketKey::new("MKT-A".to_string());
        let key_b = MarketKey::new("MKT-B".to_string());
        let key_c = MarketKey::new("MKT-C".to_string());
        let now_ms = 5_000_u64;

        // A: widest captured spread (0.10). B: narrower (0.02). C: NO book at all
        // (absent touch -> no candidate -> fail-closed exclusion). Spread-only
        // weighting with max_active_markets = 1 makes A the sole top-K market.
        seed_yes_touch(&mut maker, &key_a, 0.45, 0.55, 10.0);
        seed_yes_touch(&mut maker, &key_b, 0.49, 0.51, 10.0);
        // C left untouched: yes_book has no best_bid/ask.

        // (a) Ranked top-K: only A is active; B is ranked-but-trimmed, C excluded.
        let active = maker.active_market_set(now_ms);
        assert!(
            active.contains(&key_a),
            "the widest-spread market must rank in"
        );
        assert!(
            !active.contains(&key_b),
            "a lower-ranked market beyond max_active_markets must be excluded"
        );
        // (b) Fail-closed exclude: the market with no book never becomes active.
        assert!(
            !active.contains(&key_c),
            "a market with an absent book must be fail-closed-excluded (no candidate)"
        );
        assert_eq!(
            active.len(),
            1,
            "max_active_markets = 1 caps the active set"
        );
    }

    #[test]
    fn deselecting_a_market_cancels_its_resting_quote() {
        let mut maker = make_three_market_maker(unarmed_admission());
        let key_a = MarketKey::new("MKT-A".to_string());
        let now_ms = warm_spot_feed_for(&mut maker, &key_a);

        // Make A the sole active market and quote it: it must rest both legs.
        seed_yes_touch(&mut maker, &key_a, 0.45, 0.55, 10.0);
        run_quote_tick_with_selection(&mut maker, &key_a, now_ms);
        assert_eq!(
            maker.submit_attempts.len(),
            2,
            "the active market must translate into two leg submits"
        );
        assert!(
            maker.unit(&key_a).slot(Leg::Yes).client_order_id.is_some(),
            "the YES leg must hold a resting client order id after quoting"
        );
        let submits_after_quote = maker.submit_attempts.len();

        // Now DESELECT A by removing its book (no candidate -> not active) and run
        // its tick: the deselected market must cancel its resting quote via the
        // existing CancelAllBothLegs path (slots reset to idle) and submit nothing
        // new.
        {
            let unit = maker.unit_mut(&key_a);
            unit.yes_book = MakerLegBook::empty();
        }
        let active = maker.active_market_set(now_ms);
        assert!(
            !active.contains(&key_a),
            "removing the book must deselect the market (control for the cancel path)"
        );
        maker.run_quote_tick(&key_a, &active, now_ms);
        assert_eq!(
            maker.submit_attempts.len(),
            submits_after_quote,
            "a deselected market must submit no new quotes"
        );
        assert!(
            maker.unit(&key_a).slot(Leg::Yes).client_order_id.is_none()
                && maker.unit(&key_a).slot(Leg::No).client_order_id.is_none(),
            "cancel-on-deselect must reset both leg slots (the CancelAllBothLegs path ran)"
        );
    }

    #[test]
    fn an_invalid_selection_block_fails_loud_through_validate_config() {
        // All-zero weights are rejected by SelectionWeights::new (no meaningful
        // ranking) AND max_active_markets = 0 is below the floor — both must
        // surface as fail-loud selection errors through the builder.
        let toml = three_market_config_toml()
            .replace("spread_weight = 1.0", "spread_weight = 0.0")
            .replace("max_active_markets = 1", "max_active_markets = 0");
        let raw = toml::from_str::<Value>(&toml).expect("config still parses");
        let mut errors: Vec<ValidationError> = Vec::new();
        BinaryOracleMakerBuilder::validate_config(&raw, "strategies[0].config", &mut errors);
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("SelectionWeights::new")),
            "an all-zero weight set must fail loud via the reused guard, got: {errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|e| e.message.contains("max_active_markets")),
            "a zero max_active_markets must fail loud, got: {errors:?}"
        );
        assert!(
            errors.iter().all(|e| e.field.ends_with(".selection")),
            "selection errors must carry the .selection field, got: {errors:?}"
        );
    }
}
