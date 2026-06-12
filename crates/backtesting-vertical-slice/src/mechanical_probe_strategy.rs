//! Mechanical trade-replay order-producing probe (Gate 5 path proof).
//!
//! This bolt-owned, NautilusTrader-trait-based strategy exists for one purpose:
//! to make a trade-only TRADE_REPLAY backtest deterministically produce orders
//! and positions, proving the full
//! `data -> strategy -> orders -> positions -> result-contract` path end to end.
//! Every registered production strategy enters from `on_quote`, and trade-only
//! catalogs carry no quote ticks, so a production run yields `total_orders == 0`
//! by construction. That zero is honest for those strategies but means the
//! order/fill/position half of the engine is never exercised by a backtest.
//!
//! The probe closes that gap. It counts delivered [`TradeTick`]s and submits a
//! single market entry on the configured trade count, then a single reduce-only
//! market close after the configured number of further trades.
//!
//! No-market-reject invariant: NautilusTrader's simulated venue seeds both bid
//! and ask from the first delivered trade (`process_trade_tick`), and the
//! backtest engine routes each data point to the exchange *before* running the
//! strategy callbacks that queue the submit command, draining the command queue
//! afterward. Submitting on the Nth delivered trade for any `N >= 1` therefore
//! always finds a seeded market, and the market order fills immediately. The
//! `entry_after_trades >= 1` lower bound (enforced at manifest validation) keeps
//! that invariant intact.
//!
//! Order semantics: the orders this probe places carry **zero execution-quality
//! meaning**. Under TRADE_REPLAY fidelity the simulated venue prices fills off a
//! trade-seeded synthetic top of book, not a real order book, so fills, sizes,
//! and timing prove only that the order pathway is wired — never that any price
//! was achievable. The result-contract claim limits already forbid
//! execution-quality claims for TRADE_REPLAY sources; this probe does not, and
//! must not, weaken that.

use std::fmt::Debug;

use nautilus_common::actor::DataActor;
use nautilus_model::{
    data::TradeTick,
    enums::OrderSide,
    identifiers::{InstrumentId, StrategyId},
    types::Quantity,
};
use nautilus_trading::{
    nautilus_strategy,
    strategy::{Strategy, StrategyConfig, StrategyCore},
};

/// Configuration for the [`MechanicalTradeReplayProbe`].
///
/// Mirrors the registry-selected compiled-Rust config shape: a `base`
/// [`StrategyConfig`] plus typed required fields. There is deliberately no
/// inline code, path, or untracked blob — the manifest selects this strategy by
/// registry key and supplies these typed parameters.
#[derive(Debug, Clone)]
pub struct MechanicalTradeReplayProbeConfig {
    /// Base strategy configuration.
    pub base: StrategyConfig,
    /// Instrument to subscribe to and trade.
    pub instrument_id: InstrumentId,
    /// Order quantity for the entry and the reduce-only close.
    pub trade_size: Quantity,
    /// Number of delivered trades after which the entry order is submitted.
    pub entry_after_trades: u64,
    /// Number of further delivered trades after entry before the reduce-only close.
    pub exit_after_trades: u64,
    /// Side of the entry order; the close is submitted on the opposite side.
    pub side: OrderSide,
}

impl MechanicalTradeReplayProbeConfig {
    /// Creates a new [`MechanicalTradeReplayProbeConfig`].
    #[must_use]
    pub fn new(
        instrument_id: InstrumentId,
        trade_size: Quantity,
        entry_after_trades: u64,
        exit_after_trades: u64,
        side: OrderSide,
    ) -> Self {
        Self {
            base: StrategyConfig {
                strategy_id: Some(StrategyId::from("MECH_PROBE-001")),
                order_id_tag: Some("001".to_string()),
                ..Default::default()
            },
            instrument_id,
            trade_size,
            entry_after_trades,
            exit_after_trades,
            side,
        }
    }
}

/// Deterministic trade-driven order-producing probe over TRADE_REPLAY data.
///
/// See the module documentation for the no-market-reject invariant and the
/// zero-execution-quality meaning of the orders this strategy places.
pub struct MechanicalTradeReplayProbe {
    core: StrategyCore,
    instrument_id: InstrumentId,
    trade_size: Quantity,
    entry_after_trades: u64,
    exit_after_trades: u64,
    side: OrderSide,
    observed: u64,
    entered: bool,
    exited: bool,
}

impl MechanicalTradeReplayProbe {
    /// Creates a new [`MechanicalTradeReplayProbe`] from config.
    #[must_use]
    pub fn new(config: MechanicalTradeReplayProbeConfig) -> Self {
        Self {
            core: StrategyCore::new(config.base),
            instrument_id: config.instrument_id,
            trade_size: config.trade_size,
            entry_after_trades: config.entry_after_trades,
            exit_after_trades: config.exit_after_trades,
            side: config.side,
            observed: 0,
            entered: false,
            exited: false,
        }
    }

    fn submit_market(&mut self, side: OrderSide, reduce_only: Option<bool>) -> anyhow::Result<()> {
        let order = self.core.order_factory().market(
            self.instrument_id,
            side,
            self.trade_size,
            None,        // time_in_force
            reduce_only, // reduce_only
            None,        // quote_quantity
            None,        // exec_algorithm_id
            None,        // exec_algorithm_params
            None,        // tags
            None,        // client_order_id
        );
        self.submit_order(order, None, None, None)
    }

    fn closing_side(&self) -> OrderSide {
        match self.side {
            OrderSide::Buy => OrderSide::Sell,
            OrderSide::Sell => OrderSide::Buy,
            OrderSide::NoOrderSide => OrderSide::NoOrderSide,
        }
    }
}

nautilus_strategy!(MechanicalTradeReplayProbe);

impl Debug for MechanicalTradeReplayProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(stringify!(MechanicalTradeReplayProbe))
            .field("instrument_id", &self.instrument_id)
            .field("trade_size", &self.trade_size)
            .field("entry_after_trades", &self.entry_after_trades)
            .field("exit_after_trades", &self.exit_after_trades)
            .field("side", &self.side)
            .field("observed", &self.observed)
            .field("entered", &self.entered)
            .field("exited", &self.exited)
            .finish()
    }
}

impl DataActor for MechanicalTradeReplayProbe {
    fn on_start(&mut self) -> anyhow::Result<()> {
        self.subscribe_trades(self.instrument_id, None, None);
        Ok(())
    }

    fn on_stop(&mut self) -> anyhow::Result<()> {
        self.unsubscribe_trades(self.instrument_id, None, None);
        Ok(())
    }

    fn on_trade(&mut self, _tick: &TradeTick) -> anyhow::Result<()> {
        self.observed += 1;

        if self.observed == self.entry_after_trades && !self.entered {
            self.submit_market(self.side, None)?;
            self.entered = true;
        } else if self.entered
            && !self.exited
            && self.observed == self.entry_after_trades + self.exit_after_trades
        {
            let closing_side = self.closing_side();
            self.submit_market(closing_side, Some(true))?;
            self.exited = true;
        }

        Ok(())
    }
}
