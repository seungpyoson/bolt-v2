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
    instruments::Instrument,
    types::Quantity,
};
use nautilus_trading::{
    StrategyNative, nautilus_strategy,
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

/// Re-quantizes a requested order size to an instrument's own size precision,
/// failing loud if the size quantizes to zero.
///
/// NautilusTrader's matching engine rejects any order whose quantity precision
/// is not *exactly* the instrument's `size_precision`, and the manifest carries
/// `trade_size` as a free-form decimal whose parsed precision need not match the
/// catalog instrument's `size_increment`-derived precision. Binding the order
/// quantity through `instrument.try_make_qty` makes the probe correct for any
/// (config size, instrument precision) pair rather than only one fixture.
///
/// This is a pure free fn so it can be unit-tested directly. It deliberately is
/// **not** exercised through a `run_backtest`/`node.run()` test that asserts
/// `on_start` errors: NT's `kernel.start_trader()` swallows an `on_start` `Err`
/// (it logs the failure and does not propagate it), so a backtest-driven test
/// asserting on the `on_start` error path would be false. The error contract is
/// verified here at the helper boundary instead.
fn quantize_trade_size_to_instrument(
    instrument: &dyn Instrument,
    instrument_id: InstrumentId,
    requested: Quantity,
) -> anyhow::Result<Quantity> {
    instrument
        .try_make_qty(requested.as_f64(), None)
        .map_err(|e| {
            anyhow::anyhow!(
                "trade_size {} quantizes to zero against instrument {} \
                 (size_increment precision {}): {}",
                requested,
                instrument_id,
                instrument.size_precision(),
                e,
            )
        })
}

nautilus_strategy!(MechanicalTradeReplayProbe);

#[cfg(test)]
mod tests {
    use nautilus_model::instruments::Instrument as _;
    use nautilus_model::types::Quantity;

    use super::quantize_trade_size_to_instrument;
    use crate::catalog_projection::{SpotInstrumentSpec, build_currency_pair};

    /// Baseline spec with size_increment "0.0001" (precision 4).
    fn probe_instrument_spec() -> SpotInstrumentSpec {
        SpotInstrumentSpec {
            nt_instrument_id: "BNBUSDC.BYBIT".to_string(),
            raw_symbol: "BNBUSDC".to_string(),
            base_currency: "BNB".to_string(),
            quote_currency: "USDC".to_string(),
            price_increment: "0.1".to_string(),
            size_increment: "0.0001".to_string(),
            min_quantity: "0.0001".to_string(),
            max_quantity: "1400".to_string(),
            min_notional: "5".to_string(),
            max_notional: "200000".to_string(),
        }
    }

    /// Verifies that `try_make_qty` succeeds for a trade_size whose precision is
    /// compatible with the instrument (the guard passes, no panic).
    #[test]
    fn try_make_qty_succeeds_for_compatible_size() {
        let instrument = build_currency_pair(&probe_instrument_spec())
            .expect("build_currency_pair must not fail for valid spec");
        // trade_size "0.01" has precision 2; instrument precision 4 — fine.
        let result = instrument.try_make_qty(0.01_f64, None);
        assert!(
            result.is_ok(),
            "expected Ok for compatible trade_size, got: {result:?}"
        );
        assert!(result.unwrap().is_positive());
    }

    /// Verifies that `try_make_qty` returns an error when a sub-increment
    /// trade_size rounds to zero against the instrument's precision.
    /// This is the exact guard the `on_start` fix relies on: `try_make_qty(...)?`
    /// converts NT's bail into a clean anyhow::Error instead of an unwrap panic.
    #[test]
    fn try_make_qty_errors_for_sub_increment_size() {
        let instrument = build_currency_pair(&probe_instrument_spec())
            .expect("build_currency_pair must not fail for valid spec");
        // trade_size 1e-5 < size_increment 0.0001 (precision 4) — rounds to zero.
        let result = instrument.try_make_qty(1e-5_f64, None);
        assert!(
            result.is_err(),
            "expected Err for sub-increment trade_size, got Ok({:?})",
            result.ok()
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("rounded to zero"),
            "expected 'rounded to zero' in error, got: {msg}"
        );
    }

    /// The extracted helper fails loud when a sub-increment trade_size quantizes
    /// to zero, surfacing the probe-specific context (the "quantizes to zero
    /// against instrument" message and the instrument's size precision).
    #[test]
    fn quantize_trade_size_fails_loud_for_sub_increment_size() {
        let instrument = build_currency_pair(&probe_instrument_spec())
            .expect("build_currency_pair must not fail for valid spec");
        let instrument_id = instrument.id();
        // "0.00001" has precision 5 < instrument size_increment 0.0001
        // (precision 4) — it quantizes to zero against this instrument.
        let requested = Quantity::from("0.00001");
        let error = quantize_trade_size_to_instrument(&instrument, instrument_id, requested)
            .expect_err("sub-increment trade_size must fail loud");
        let msg = error.to_string();
        assert!(
            msg.contains("quantizes to zero against instrument"),
            "expected 'quantizes to zero against instrument' in error, got: {msg}"
        );
        // The instrument's size precision (4) must be reported so the operator
        // can see what precision the configured size was measured against.
        assert!(
            msg.contains(&format!(
                "size_increment precision {}",
                instrument.size_precision()
            )),
            "expected instrument size precision {} in error, got: {msg}",
            instrument.size_precision()
        );
    }

    /// The extracted helper quantizes a compatible trade_size and the returned
    /// quantity adopts the instrument's own size precision, which is what the
    /// matching engine requires.
    #[test]
    fn quantize_trade_size_adopts_instrument_size_precision() {
        let instrument = build_currency_pair(&probe_instrument_spec())
            .expect("build_currency_pair must not fail for valid spec");
        let instrument_id = instrument.id();
        // "0.01" has parsed precision 2; the instrument's size precision is 4.
        let requested = Quantity::from("0.01");
        assert_ne!(
            requested.precision,
            instrument.size_precision(),
            "test premise: requested precision must differ from instrument precision"
        );
        let quantized = quantize_trade_size_to_instrument(&instrument, instrument_id, requested)
            .expect("compatible trade_size must quantize");
        assert!(quantized.is_positive(), "quantized size must be positive");
        assert_eq!(
            quantized.precision,
            instrument.size_precision(),
            "quantized size must adopt the instrument's size precision"
        );
    }
}

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
        // Re-quantize the configured order size to the instrument's own size
        // precision. NautilusTrader's matching engine rejects any order whose
        // quantity precision is not *exactly* the instrument's `size_precision`
        // (`OrderMatchingEngine::process_order`: "Invalid order quantity
        // precision"). The manifest carries `trade_size` as a free-form decimal
        // string whose parsed precision (e.g. `0.01` -> 2) need not match the
        // catalog instrument's `size_increment`-derived precision (e.g.
        // `0.0001` -> 4), so the raw config quantity is not admissible. Binding
        // the order quantity to `instrument.make_qty` makes the probe correct
        // for any (config size, instrument precision) pair rather than only the
        // one fixture, and fails loud if the instrument is missing.
        let trade_size = {
            let cache = self.cache();
            let instrument = cache.instrument(&self.instrument_id).ok_or_else(|| {
                anyhow::anyhow!(
                    "instrument {} not found in cache for mechanical probe",
                    self.instrument_id
                )
            })?;
            quantize_trade_size_to_instrument(&instrument, self.instrument_id, self.trade_size)?
        };
        self.trade_size = trade_size;

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
