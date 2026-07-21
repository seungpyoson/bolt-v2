//! Exact economic validation for opted-in historical backtest diagnostics.
//!
//! This module does not model execution. It compares the runner's observed
//! economic trace with results produced by NautilusTrader's shared order-book,
//! instrument sizing, position, and account primitives.
//!
//! Configuration provenance is an integrity agreement between canonical
//! resolved bytes and their recorded hash. It is not a frozen configuration
//! golden; applied-override sensitivity is verified by the configuration owner.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail, ensure};
use nautilus_model::{
    enums::{LiquiditySide, OrderSide, OrderType, PositionSide},
    events::{OrderFilled, OrderUpdated},
    identifiers::{AccountId, ClientOrderId, InstrumentId, PositionId, StrategyId, TraderId},
    instruments::{Instrument, InstrumentAny},
    orderbook::OrderBook,
    types::{Currency, Money, Price, Quantity},
};
use rust_decimal::{
    Decimal, RoundingStrategy,
    prelude::{Signed, ToPrimitive},
};

use crate::hashing::sha256_hex;

#[derive(Clone, Debug)]
pub struct SubmittedOrderTrace {
    pub trader_id: TraderId,
    pub strategy_id: StrategyId,
    pub instrument_id: InstrumentId,
    pub client_order_id: ClientOrderId,
    pub account_id: AccountId,
    pub order_side: OrderSide,
    pub order_type: OrderType,
    pub quantity: Quantity,
    pub quote_quantity: bool,
    pub post_only: bool,
    pub reconciliation: bool,
}

#[derive(Clone)]
pub enum ExecutionOrderCause {
    Submitted {
        executable_book: Box<OrderBook>,
        submitted_order: SubmittedOrderTrace,
        quote_conversion: Option<Box<OrderUpdated>>,
    },
    Settlement {
        declared_price: Price,
    },
}

#[derive(Clone)]
pub struct ExecutionOrderTrace {
    pub cause: ExecutionOrderCause,
    pub fills: Vec<OrderFilled>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionEffectKind {
    Opened,
    Changed,
    Closed,
}

#[derive(Clone)]
pub struct PositionEffectTrace {
    pub kind: PositionEffectKind,
    pub trader_id: TraderId,
    pub strategy_id: StrategyId,
    pub position_id: PositionId,
    pub instrument_id: InstrumentId,
    pub account_id: AccountId,
    pub opening_order_id: ClientOrderId,
    pub closing_order_id: Option<ClientOrderId>,
    pub entry: OrderSide,
    pub side: PositionSide,
    pub signed_quantity: f64,
    pub quantity: Quantity,
    pub last_quantity: Quantity,
    pub last_price: Price,
    pub currency: Currency,
    pub realized_pnl: Option<Money>,
}

pub struct ExecutionContractTrace<'a> {
    pub instrument: &'a InstrumentAny,
    pub configured_account_id: AccountId,
    pub orders: Vec<ExecutionOrderTrace>,
    pub position_effects: Vec<PositionEffectTrace>,
    pub initial_cash: Money,
    pub account_cash_after_fills: Vec<Money>,
    pub terminal_cash: Money,
    pub realized_pnl: Money,
    pub position_commissions: Vec<Money>,
    pub canonical_resolved_config_bytes: &'a [u8],
    pub canonical_resolved_config_sha256: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionContractReport {
    pub validated_fill_count: usize,
    pub entry_fill_count: usize,
    pub normal_exit_fill_count: usize,
    pub settlement_fill_count: usize,
}

/// Validate an observed trace against shared NautilusTrader economics.
///
pub fn validate_execution_contract(
    trace: &ExecutionContractTrace<'_>,
) -> Result<ExecutionContractReport> {
    ensure!(
        sha256_hex(trace.canonical_resolved_config_bytes) == trace.canonical_resolved_config_sha256,
        "canonical resolved configuration bytes do not match recorded provenance"
    );

    ensure!(
        matches!(trace.instrument, InstrumentAny::BinaryOption(_)),
        "#789 complete lifecycle is restricted to one binary-option instrument"
    );
    ensure!(
        !trace.orders.is_empty(),
        "execution contract requires orders"
    );
    let size_precision = trace.instrument.size_precision();
    let size_increment = trace.instrument.size_increment().as_decimal();
    let price_precision = trace.instrument.price_precision();
    let price_increment = trace.instrument.price_increment().as_decimal();
    let multiplier = trace.instrument.multiplier().as_decimal();
    ensure!(
        trace.instrument.taker_fee().is_zero(),
        "#789 lifecycle evidence is restricted to the instrument's zero taker-fee configuration"
    );
    ensure!(
        trace.initial_cash.currency == trace.realized_pnl.currency
            && trace.terminal_cash.currency == trace.realized_pnl.currency,
        "#789 cash and realized PnL currencies must agree"
    );
    let mut trade_ids = BTreeSet::new();
    let mut client_order_ids = BTreeSet::new();
    let mut venue_order_ids = BTreeSet::new();
    let mut position_id = None;
    let mut opening_order_id = None;
    let mut entry_side = None;
    let mut lifecycle_account_id = None;
    let mut exposure = Decimal::ZERO;
    let mut average_open = Decimal::ZERO;
    let mut derived_pnl = Decimal::ZERO;
    let mut entry_fill_count = 0;
    let mut normal_exit_fill_count = 0;
    let mut settlement_fill_count = 0;
    let mut position_effect_index = 0usize;
    let mut derived_cash = trace.initial_cash.as_decimal();
    let mut derived_commissions = BTreeMap::<String, Decimal>::new();

    for (order_index, order) in trace.orders.iter().enumerate() {
        let first_fill = order
            .fills
            .first()
            .with_context(|| format!("execution order {order_index} has no fills"))?;
        ensure!(
            client_order_ids.insert(first_fill.client_order_id)
                && venue_order_ids.insert(first_fill.venue_order_id),
            "lifecycle order identity partition is not unique across entry, reduction, and settlement"
        );
        let side = match &order.cause {
            ExecutionOrderCause::Submitted {
                submitted_order, ..
            } => {
                ensure!(
                    submitted_order.account_id == trace.configured_account_id,
                    "normal order diverges from the configured account anchor"
                );
                ensure!(
                    order
                        .fills
                        .iter()
                        .all(|fill| fill.account_id == submitted_order.account_id),
                    "normal fills diverge from the submitted account anchor"
                );
                ensure!(
                    submitted_order.instrument_id == trace.instrument.id()
                        && submitted_order.order_side != OrderSide::NoOrderSide
                        && submitted_order.order_type == OrderType::Market
                        && !submitted_order.post_only
                        && !submitted_order.reconciliation
                        && order.fills.iter().all(|fill| {
                            fill.trader_id == submitted_order.trader_id
                                && fill.strategy_id == submitted_order.strategy_id
                                && fill.instrument_id == submitted_order.instrument_id
                                && fill.client_order_id == submitted_order.client_order_id
                                && fill.order_side == submitted_order.order_side
                                && fill.order_type == submitted_order.order_type
                        }),
                    "normal fills diverge from submitted order semantics"
                );
                match lifecycle_account_id {
                    Some(expected) => ensure!(
                        expected == submitted_order.account_id,
                        "normal order diverges from the submitted account anchor"
                    ),
                    None => lifecycle_account_id = Some(submitted_order.account_id),
                }
                submitted_order.order_side
            }
            ExecutionOrderCause::Settlement { .. } => {
                ensure!(
                    lifecycle_account_id == Some(first_fill.account_id),
                    "settlement fill diverges from the submitted account anchor"
                );
                first_fill.order_side
            }
        };
        ensure!(side != OrderSide::NoOrderSide, "fill has no specified side");
        for fill in &order.fills {
            ensure!(
                fill.instrument_id == trace.instrument.id()
                    && fill.trader_id == first_fill.trader_id
                    && fill.strategy_id == first_fill.strategy_id
                    && fill.order_type == OrderType::Market
                    && fill.liquidity_side == LiquiditySide::Taker
                    && fill.order_side == side
                    && fill.client_order_id == first_fill.client_order_id
                    && fill.venue_order_id == first_fill.venue_order_id
                    && fill.account_id == first_fill.account_id
                    && fill.currency == trace.realized_pnl.currency
                    && !fill.reconciliation,
                "execution order fills must share one market-order identity"
            );
            ensure!(
                fill.last_qty.precision == size_precision
                    && (fill.last_qty.as_decimal() % size_increment).is_zero(),
                "fill quantity {} does not use instrument size precision {} and increment {}",
                fill.last_qty,
                size_precision,
                trace.instrument.size_increment()
            );
            ensure!(
                fill.last_px.precision == price_precision
                    && (fill.last_px.as_decimal() % price_increment).is_zero(),
                "fill price {} does not use instrument price precision {} and increment {}",
                fill.last_px,
                price_precision,
                trace.instrument.price_increment()
            );
            ensure!(
                trade_ids.insert(fill.trade_id),
                "duplicate trade ID {} in ordered lifecycle",
                fill.trade_id
            );
        }

        if let ExecutionOrderCause::Submitted {
            executable_book,
            submitted_order,
            quote_conversion,
        } = &order.cause
        {
            ensure!(
                executable_book.instrument_id == trace.instrument.id(),
                "executable book instrument does not match the lifecycle instrument"
            );
            ensure!(
                settlement_fill_count == 0,
                "submitted normal order appears after settlement"
            );
            let requested_base = normalized_base_quantity(
                trace.instrument,
                executable_book,
                side,
                submitted_order.quantity,
                submitted_order.quote_quantity,
            )?;
            match (submitted_order.quote_quantity, quote_conversion) {
                (true, Some(update)) => {
                    ensure!(
                        update.trader_id == submitted_order.trader_id
                            && update.strategy_id == submitted_order.strategy_id
                            && update.instrument_id == submitted_order.instrument_id
                            && update.client_order_id == submitted_order.client_order_id,
                        "quote conversion witness identity diverges from submitted order"
                    );
                    ensure!(
                        update.quantity == requested_base,
                        "quote conversion witness quantity {} diverges from independently normalized {}",
                        update.quantity,
                        requested_base
                    );
                    ensure!(
                        update.venue_order_id.is_none(),
                        "quote conversion witness unexpectedly has venue-order identity {:?}",
                        update.venue_order_id
                    );
                    ensure!(
                        update.account_id == Some(submitted_order.account_id),
                        "quote conversion witness account identity {:?} diverges from submitted account {}",
                        update.account_id,
                        submitted_order.account_id
                    );
                    ensure!(
                        update.price.is_none(),
                        "quote conversion witness unexpectedly has price {:?}",
                        update.price
                    );
                    ensure!(
                        update.trigger_price.is_none(),
                        "quote conversion witness unexpectedly has trigger price {:?}",
                        update.trigger_price
                    );
                    ensure!(
                        update.protection_price.is_none(),
                        "quote conversion witness unexpectedly has protection price {:?}",
                        update.protection_price
                    );
                    ensure!(
                        !update.is_quote_quantity && !update.reconciliation,
                        "quote conversion witness retains quote or reconciliation flags"
                    );
                }
                (true, None) => bail!("quote submission lacks its quote conversion witness"),
                (false, Some(_)) => {
                    bail!("base-denominated submission has an unexpected quote conversion witness")
                }
                (false, None) => {}
            }
            let expected = independent_market_sweep(executable_book, side, requested_base)?;
            let observed_quantity = order
                .fills
                .iter()
                .map(|fill| fill.last_qty.as_decimal())
                .sum::<Decimal>();
            ensure!(
                expected.len() == order.fills.len()
                    && expected
                        .iter()
                        .zip(&order.fills)
                        .all(|((price, quantity), fill)| {
                            *price == fill.last_px.as_decimal()
                                && *quantity == fill.last_qty.as_decimal()
                        }),
                "observed normal-order fills do not equal the executable book at submission"
            );
            ensure!(
                observed_quantity == requested_base.as_decimal(),
                "normal-order fill quantity does not equal the effective submitted quantity"
            );
        }

        let before = exposure;
        for fill in &order.fills {
            let before_fill = exposure;
            let quantity = fill.last_qty.as_decimal();
            let price = fill.last_px.as_decimal();
            let signed_quantity = match side {
                OrderSide::Buy => quantity,
                OrderSide::Sell => -quantity,
                OrderSide::NoOrderSide => unreachable!(),
            };
            if exposure.is_zero() || exposure.signum() == signed_quantity.signum() {
                let old_abs = exposure.abs();
                let new_abs = old_abs + quantity;
                average_open = if old_abs.is_zero() {
                    price
                } else {
                    ((average_open * old_abs) + (price * quantity)) / new_abs
                };
                exposure += signed_quantity;
            } else {
                ensure!(
                    quantity <= exposure.abs(),
                    "normal fill reverses or reopens the position"
                );
                let points = if exposure.is_sign_positive() {
                    price - average_open
                } else {
                    average_open - price
                };
                derived_pnl += points * quantity * multiplier;
                exposure += signed_quantity;
            }

            let commission = fill.commission.with_context(|| {
                format!(
                    "missing commission evidence for lifecycle fill {}",
                    fill.trade_id
                )
            })?;
            ensure!(
                commission.as_decimal().is_zero(),
                "fill commission does not match the instrument's zero taker fee"
            );
            ensure!(
                commission.currency == trace.realized_pnl.currency,
                "#789 commission currency differs from settlement currency"
            );
            *derived_commissions
                .entry(commission.currency.to_string())
                .or_default() += commission.as_decimal();
            derived_pnl -= commission.as_decimal();
            let notional = price * quantity * multiplier;
            derived_cash += match side {
                OrderSide::Buy => -notional,
                OrderSide::Sell => notional,
                OrderSide::NoOrderSide => unreachable!(),
            };
            derived_cash -= commission.as_decimal();
            let expected_cash = Money::from_decimal(derived_cash, trace.realized_pnl.currency)
                .map_err(anyhow::Error::msg)
                .context("intermediate lifecycle cash is not representable")?;
            let observed_cash = trace
                .account_cash_after_fills
                .get(position_effect_index)
                .with_context(|| {
                    format!(
                        "missing AccountState after lifecycle fill {}",
                        fill.trade_id
                    )
                })?;
            ensure!(
                *observed_cash == expected_cash,
                "AccountState cash does not equal the independent per-fill cash fold"
            );

            let effect = trace
                .position_effects
                .get(position_effect_index)
                .with_context(|| {
                    format!(
                        "missing position mutation for lifecycle fill {}",
                        fill.trade_id
                    )
                })?;
            position_effect_index += 1;
            ensure!(
                effect.quantity.precision == size_precision
                    && effect.last_quantity.precision == size_precision
                    && (effect.quantity.as_decimal() % size_increment).is_zero()
                    && (effect.last_quantity.as_decimal() % size_increment).is_zero(),
                "position mutation quantity does not use instrument size precision and increment"
            );
            ensure!(
                effect.last_price.precision == price_precision
                    && (effect.last_price.as_decimal() % price_increment).is_zero(),
                "position mutation price does not use instrument price precision and increment"
            );
            let expected_effect_kind = if before_fill.is_zero() && !exposure.is_zero() {
                PositionEffectKind::Opened
            } else if !before_fill.is_zero() && exposure.is_zero() {
                PositionEffectKind::Closed
            } else {
                PositionEffectKind::Changed
            };
            let expected_opening_order_id = *opening_order_id.get_or_insert(fill.client_order_id);
            let expected_entry_side = *entry_side.get_or_insert(fill.order_side);
            let expected_closing_order_id = (expected_effect_kind == PositionEffectKind::Closed)
                .then_some(fill.client_order_id);
            let expected_signed_quantity = exposure
                .to_f64()
                .context("folded position quantity is not representable as f64")?;
            ensure!(
                effect.trader_id == fill.trader_id
                    && effect.strategy_id == fill.strategy_id
                    && effect.instrument_id == fill.instrument_id
                    && effect.account_id == fill.account_id
                    && effect.opening_order_id == expected_opening_order_id
                    && effect.closing_order_id == expected_closing_order_id
                    && effect.entry == expected_entry_side
                    && effect.signed_quantity == expected_signed_quantity
                    && effect.last_quantity == fill.last_qty
                    && effect.last_price == fill.last_px
                    && effect.currency == fill.currency,
                "position mutation does not identify its causal lifecycle"
            );
            match position_id {
                Some(expected) => ensure!(
                    expected == effect.position_id,
                    "ordered lifecycle spans multiple position IDs"
                ),
                None => position_id = Some(effect.position_id),
            }
            let observed_exposure = signed_position_quantity(effect.side, effect.quantity)?;
            ensure!(
                observed_exposure == exposure,
                "position mutation quantity does not equal independently folded exposure"
            );
            ensure!(
                effect.kind == expected_effect_kind,
                "position mutation kind does not match its independently derived position effect"
            );
            let expected_effect_pnl = Money::from_decimal(derived_pnl, trace.realized_pnl.currency)
                .map_err(anyhow::Error::msg)
                .context("intermediate lifecycle PnL is not representable")?;
            let observed_effect_pnl = effect
                .realized_pnl
                .unwrap_or_else(|| Money::zero(trace.realized_pnl.currency));
            ensure!(
                observed_effect_pnl == expected_effect_pnl,
                "position mutation realized PnL does not equal the independent lifecycle fold"
            );
        }

        match &order.cause {
            ExecutionOrderCause::Submitted {
                submitted_order, ..
            } if before.is_zero() && !exposure.is_zero() => {
                ensure!(
                    submitted_order.quote_quantity,
                    "#789 entry must be quote-denominated"
                );
                ensure!(entry_fill_count == 0, "lifecycle contains a second entry");
                entry_fill_count = order.fills.len();
            }
            ExecutionOrderCause::Submitted {
                submitted_order, ..
            } if !before.is_zero()
                && (exposure.is_zero()
                    || (before.signum() == exposure.signum() && exposure.abs() < before.abs())) =>
            {
                ensure!(
                    !submitted_order.quote_quantity,
                    "#789 reduction must be base-denominated"
                );
                ensure!(
                    normal_exit_fill_count == 0,
                    "#789 lifecycle is restricted to a single normal reduction order"
                );
                ensure!(
                    !exposure.is_zero(),
                    "normal exit closed the position before required settlement"
                );
                normal_exit_fill_count += order.fills.len();
            }
            ExecutionOrderCause::Submitted { .. } => {
                bail!("submitted order does not have entry or reducing position effect")
            }
            ExecutionOrderCause::Settlement { declared_price } => {
                ensure!(
                    order.fills.len() == 1,
                    "#789 settlement must contain exactly one fill"
                );
                ensure!(
                    declared_price.precision == price_precision
                        && (declared_price.as_decimal() % price_increment).is_zero(),
                    "declared settlement price does not use instrument price precision and increment"
                );
                ensure!(
                    order_index + 1 == trace.orders.len()
                        && settlement_fill_count == 0
                        && !before.is_zero()
                        && exposure.is_zero(),
                    "settlement must be the single final order and exactly close the remainder"
                );
                ensure!(
                    order
                        .fills
                        .iter()
                        .all(|fill| fill.last_px == *declared_price),
                    "settlement fill price does not equal the declared close price"
                );
                settlement_fill_count = order.fills.len();
            }
        }
    }

    ensure!(
        entry_fill_count > 0
            && normal_exit_fill_count > 0
            && settlement_fill_count > 0
            && exposure.is_zero(),
        "complete lifecycle requires entry, normal exit, and exact settlement close"
    );
    ensure!(
        position_effect_index == trace.position_effects.len(),
        "position evidence contains mutations without causal fills"
    );
    ensure!(
        position_effect_index == trace.account_cash_after_fills.len(),
        "account evidence contains transitions without causal fills"
    );
    let replayed_pnl = Money::from_decimal(derived_pnl, trace.realized_pnl.currency)
        .map_err(anyhow::Error::msg)
        .context("derived lifecycle PnL is not representable")?;
    ensure!(
        replayed_pnl == trace.realized_pnl,
        "cached realized PnL does not equal independently folded lifecycle PnL"
    );

    let cash_change = trace
        .terminal_cash
        .checked_sub(trace.initial_cash)
        .context("terminal cash subtraction overflow or scale mismatch")?;
    ensure!(
        cash_change == replayed_pnl,
        "terminal cash change does not equal PnL replayed from typed fills"
    );

    let mut terminal_commissions = BTreeMap::<String, Decimal>::new();
    for commission in &trace.position_commissions {
        ensure!(
            terminal_commissions
                .insert(commission.currency.to_string(), commission.as_decimal())
                .is_none(),
            "terminal position commission map contains a duplicate currency"
        );
    }
    ensure!(
        terminal_commissions == derived_commissions,
        "terminal position commission map does not equal explicit per-fill commissions"
    );

    Ok(ExecutionContractReport {
        validated_fill_count: trade_ids.len(),
        entry_fill_count,
        normal_exit_fill_count,
        settlement_fill_count,
    })
}

fn signed_position_quantity(side: PositionSide, quantity: Quantity) -> Result<Decimal> {
    match side {
        PositionSide::Long => Ok(quantity.as_decimal()),
        PositionSide::Short => Ok(-quantity.as_decimal()),
        PositionSide::Flat => {
            ensure!(
                quantity.is_zero(),
                "flat position mutation carries non-zero quantity"
            );
            Ok(Decimal::ZERO)
        }
        PositionSide::NoPositionSide => bail!("position mutation has no specified side"),
    }
}

fn normalized_base_quantity(
    instrument: &InstrumentAny,
    book: &OrderBook,
    side: OrderSide,
    submitted_quantity: Quantity,
    quote_quantity: bool,
) -> Result<Quantity> {
    ensure!(
        submitted_quantity.precision == instrument.size_precision(),
        "submitted quantity precision {} does not match instrument size precision {}",
        submitted_quantity.precision,
        instrument.size_precision()
    );
    let best_price = match side {
        OrderSide::Buy => book.best_ask_price(),
        OrderSide::Sell => book.best_bid_price(),
        OrderSide::NoOrderSide => None,
    }
    .context("executable book has no opposing price")?;
    if !quote_quantity {
        ensure!(
            submitted_quantity.precision == instrument.size_precision()
                && (submitted_quantity.as_decimal() % instrument.size_increment().as_decimal())
                    .is_zero(),
            "base-denominated quantity does not use instrument precision and increment"
        );
        return Ok(submitted_quantity);
    }

    let increment = instrument.size_increment().as_decimal();
    let increment_precision = increment.normalize().scale();
    let best_price = best_price.as_decimal();
    ensure!(
        best_price > Decimal::ZERO,
        "quote conversion price must be strictly positive"
    );
    let normalized = submitted_quantity
        .as_decimal()
        .checked_div(best_price)
        .context("quote quantity division overflow")?
        .round_dp_with_strategy(increment_precision, RoundingStrategy::MidpointNearestEven);
    ensure!(
        (normalized % increment).is_zero(),
        "normalized quote quantity is not aligned to instrument size increment"
    );
    Quantity::from_decimal_dp(normalized, instrument.size_precision())
        .map_err(anyhow::Error::msg)
        .context("normalized quote quantity is not representable")
}

fn independent_market_sweep(
    book: &OrderBook,
    side: OrderSide,
    requested: Quantity,
) -> Result<Vec<(Decimal, Decimal)>> {
    let levels = match side {
        OrderSide::Buy => book.asks_as_map(None),
        OrderSide::Sell => book.bids_as_map(None),
        OrderSide::NoOrderSide => bail!("market sweep has no specified side"),
    };
    let mut remaining = requested.as_decimal();
    let mut fills = Vec::new();
    for (price, available) in levels {
        if remaining.is_zero() {
            break;
        }
        let quantity = remaining.min(available);
        if !quantity.is_zero() {
            fills.push((price, quantity));
            remaining -= quantity;
        }
    }
    ensure!(
        remaining.is_zero(),
        "insufficient executable depth for the complete normal-order quantity"
    );
    Ok(fills)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_core::UnixNanos;
    use nautilus_model::{
        data::BookOrder,
        enums::{BookType, LiquiditySide},
        identifiers::{
            AccountId, ClientOrderId, PositionId, StrategyId, TradeId, TraderId, VenueOrderId,
        },
        instruments::stubs,
        types::Currency,
    };

    type OrderUpdatedMutation = Box<dyn Fn(&mut OrderUpdated)>;

    struct Fixture {
        instrument: InstrumentAny,
        orders: Vec<ExecutionOrderTrace>,
        position_effects: Vec<PositionEffectTrace>,
        initial_cash: Money,
        account_cash_after_fills: Vec<Money>,
        terminal_cash: Money,
        realized_pnl: Money,
        position_commissions: Vec<Money>,
        config_bytes: Vec<u8>,
        config_hash: String,
    }

    impl Fixture {
        fn trace(&self) -> ExecutionContractTrace<'_> {
            ExecutionContractTrace {
                instrument: &self.instrument,
                configured_account_id: AccountId::from("POLYMARKET-001"),
                orders: self.orders.clone(),
                position_effects: self.position_effects.clone(),
                initial_cash: self.initial_cash,
                account_cash_after_fills: self.account_cash_after_fills.clone(),
                terminal_cash: self.terminal_cash,
                realized_pnl: self.realized_pnl,
                position_commissions: self.position_commissions.clone(),
                canonical_resolved_config_bytes: &self.config_bytes,
                canonical_resolved_config_sha256: &self.config_hash,
            }
        }
    }

    fn fixture() -> Fixture {
        let instrument = InstrumentAny::BinaryOption(stubs::binary_option());
        let instrument_id = instrument.id();
        let config_bytes = br#"{"order_type":"MARKET","quote_quantity":true}"#.to_vec();
        let config_hash = sha256_hex(&config_bytes);
        let position_id = PositionId::from("P-001");
        let entry_fill = test_fill(
            instrument_id,
            position_id,
            OrderSide::Buy,
            "entry",
            "0.420",
            "2.71",
            1,
        );
        let normal_exit = test_fill(
            instrument_id,
            position_id,
            OrderSide::Sell,
            "normal-exit",
            "0.430",
            "2.00",
            2,
        );
        let settlement = test_fill(
            instrument_id,
            position_id,
            OrderSide::Sell,
            "settlement",
            "1.000",
            "0.71",
            3,
        );
        let realized_pnl = Money::from("0.43180000 USDC");
        let initial_cash = Money::from("1000000.00000000 USDC");
        let terminal_cash = initial_cash
            .checked_add(realized_pnl)
            .expect("fixture cash addition should be exact");
        let entry_conversion = quote_conversion(&entry_fill, Quantity::from("2.71"));
        Fixture {
            instrument,
            orders: vec![
                ExecutionOrderTrace {
                    cause: ExecutionOrderCause::Submitted {
                        executable_book: Box::new(one_level_book(
                            instrument_id,
                            OrderSide::Sell,
                            "0.420",
                            "21.52",
                            1,
                        )),
                        submitted_order: submitted_order(&entry_fill, Quantity::from("1.14"), true),
                        quote_conversion: Some(Box::new(entry_conversion)),
                    },
                    fills: vec![entry_fill],
                },
                ExecutionOrderTrace {
                    cause: ExecutionOrderCause::Submitted {
                        executable_book: Box::new(one_level_book(
                            instrument_id,
                            OrderSide::Buy,
                            "0.430",
                            "2.00",
                            2,
                        )),
                        submitted_order: submitted_order(
                            &normal_exit,
                            Quantity::from("2.00"),
                            false,
                        ),
                        quote_conversion: None,
                    },
                    fills: vec![normal_exit],
                },
                ExecutionOrderTrace {
                    cause: ExecutionOrderCause::Settlement {
                        declared_price: Price::from("1.000"),
                    },
                    fills: vec![settlement],
                },
            ],
            position_effects: vec![
                PositionEffectTrace {
                    kind: PositionEffectKind::Opened,
                    trader_id: TraderId::from("TRADER-001"),
                    strategy_id: StrategyId::from("STRATEGY-001"),
                    position_id,
                    instrument_id,
                    account_id: AccountId::from("POLYMARKET-001"),
                    opening_order_id: ClientOrderId::from("O-entry"),
                    closing_order_id: None,
                    entry: OrderSide::Buy,
                    side: PositionSide::Long,
                    signed_quantity: 2.71,
                    quantity: Quantity::from("2.71"),
                    last_quantity: Quantity::from("2.71"),
                    last_price: Price::from("0.420"),
                    currency: Currency::USDC(),
                    realized_pnl: None,
                },
                PositionEffectTrace {
                    kind: PositionEffectKind::Changed,
                    trader_id: TraderId::from("TRADER-001"),
                    strategy_id: StrategyId::from("STRATEGY-001"),
                    position_id,
                    instrument_id,
                    account_id: AccountId::from("POLYMARKET-001"),
                    opening_order_id: ClientOrderId::from("O-entry"),
                    closing_order_id: None,
                    entry: OrderSide::Buy,
                    side: PositionSide::Long,
                    signed_quantity: 0.71,
                    quantity: Quantity::from("0.71"),
                    last_quantity: Quantity::from("2.00"),
                    last_price: Price::from("0.430"),
                    currency: Currency::USDC(),
                    realized_pnl: Some(Money::from("0.02000000 USDC")),
                },
                PositionEffectTrace {
                    kind: PositionEffectKind::Closed,
                    trader_id: TraderId::from("TRADER-001"),
                    strategy_id: StrategyId::from("STRATEGY-001"),
                    position_id,
                    instrument_id,
                    account_id: AccountId::from("POLYMARKET-001"),
                    opening_order_id: ClientOrderId::from("O-entry"),
                    closing_order_id: Some(ClientOrderId::from("O-settlement")),
                    entry: OrderSide::Buy,
                    side: PositionSide::Flat,
                    signed_quantity: 0.0,
                    quantity: Quantity::from("0.00"),
                    last_quantity: Quantity::from("0.71"),
                    last_price: Price::from("1.000"),
                    currency: Currency::USDC(),
                    realized_pnl: Some(realized_pnl),
                },
            ],
            account_cash_after_fills: vec![
                Money::from("999998.86180000 USDC"),
                Money::from("999999.72180000 USDC"),
                terminal_cash,
            ],
            initial_cash,
            terminal_cash,
            realized_pnl,
            position_commissions: vec![Money::from("0.00000000 USDC")],
            config_bytes,
            config_hash,
        }
    }

    fn test_fill(
        instrument_id: nautilus_model::identifiers::InstrumentId,
        position_id: PositionId,
        side: OrderSide,
        trade_id: &str,
        price: &str,
        quantity: &str,
        timestamp: u64,
    ) -> OrderFilled {
        OrderFilled::new(
            TraderId::from("TRADER-001"),
            StrategyId::from("STRATEGY-001"),
            instrument_id,
            ClientOrderId::from(format!("O-{trade_id}").as_str()),
            VenueOrderId::from(format!("V-{trade_id}").as_str()),
            AccountId::from("POLYMARKET-001"),
            TradeId::from(trade_id),
            side,
            OrderType::Market,
            Quantity::from(quantity),
            Price::from(price),
            Currency::USDC(),
            LiquiditySide::Taker,
            nautilus_core::UUID4::new(),
            UnixNanos::from(timestamp),
            UnixNanos::from(timestamp),
            false,
            Some(position_id),
            Some(Money::from("0.00000000 USDC")),
            None,
        )
    }

    fn submitted_order(
        fill: &OrderFilled,
        quantity: Quantity,
        quote_quantity: bool,
    ) -> SubmittedOrderTrace {
        SubmittedOrderTrace {
            trader_id: fill.trader_id,
            strategy_id: fill.strategy_id,
            instrument_id: fill.instrument_id,
            client_order_id: fill.client_order_id,
            account_id: fill.account_id,
            order_side: fill.order_side,
            order_type: fill.order_type,
            quantity,
            quote_quantity,
            post_only: false,
            reconciliation: false,
        }
    }

    fn quote_conversion(fill: &OrderFilled, quantity: Quantity) -> OrderUpdated {
        OrderUpdated::new(
            fill.trader_id,
            fill.strategy_id,
            fill.instrument_id,
            fill.client_order_id,
            quantity,
            nautilus_core::UUID4::new(),
            fill.ts_event,
            fill.ts_init,
            false,
            None,
            Some(fill.account_id),
            None,
            None,
            None,
            false,
        )
    }

    fn one_level_book(
        instrument_id: nautilus_model::identifiers::InstrumentId,
        side: OrderSide,
        price: &str,
        quantity: &str,
        timestamp: u64,
    ) -> OrderBook {
        let mut book = OrderBook::new(instrument_id, BookType::L2_MBP);
        book.add(
            BookOrder::new(
                side,
                Price::from(price),
                Quantity::from(quantity),
                timestamp,
            ),
            0,
            timestamp,
            UnixNanos::from(timestamp),
        );
        book
    }

    #[test]
    fn accepts_exact_shared_primitive_trace() {
        let report = validate_execution_contract(&fixture().trace()).expect("valid trace");
        assert_eq!(report.validated_fill_count, 3);
        assert_eq!(report.entry_fill_count, 1);
        assert_eq!(report.normal_exit_fill_count, 1);
        assert_eq!(report.settlement_fill_count, 1);
    }

    #[test]
    fn rejects_base_denominated_entry_role() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            submitted_order,
            quote_conversion,
            ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("fixture entry order changed")
        };
        submitted_order.quantity = Quantity::from("2.71");
        submitted_order.quote_quantity = false;
        *quote_conversion = None;

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("the entry role must remain quote-denominated");
        assert!(
            error
                .to_string()
                .contains("entry must be quote-denominated")
        );
    }

    #[test]
    fn rejects_quote_denominated_reduction_role() {
        let mut fixture = fixture();
        let reduction_fill = fixture.orders[1].fills[0].clone();
        let ExecutionOrderCause::Submitted {
            submitted_order,
            quote_conversion: conversion,
            ..
        } = &mut fixture.orders[1].cause
        else {
            panic!("fixture reduction order changed")
        };
        submitted_order.quantity = Quantity::from("0.86");
        submitted_order.quote_quantity = true;
        *conversion = Some(Box::new(quote_conversion(
            &reduction_fill,
            Quantity::from("2.00"),
        )));

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("the reduction role must remain base-denominated");
        assert!(
            error
                .to_string()
                .contains("reduction must be base-denominated")
        );
    }

    #[test]
    fn rejects_multiple_settlement_fills() {
        let mut fixture = fixture();
        let original = fixture.orders[2].fills[0].clone();
        let mut first = original.clone();
        first.last_qty = Quantity::from("0.30");
        first.trade_id = TradeId::from("settlement-1");
        first.event_id = nautilus_core::UUID4::new();
        let mut second = original;
        second.last_qty = Quantity::from("0.41");
        second.trade_id = TradeId::from("settlement-2");
        second.event_id = nautilus_core::UUID4::new();
        fixture.orders[2].fills = vec![first, second];

        fixture.position_effects.pop();
        fixture.position_effects.extend([
            PositionEffectTrace {
                kind: PositionEffectKind::Changed,
                trader_id: TraderId::from("TRADER-001"),
                strategy_id: StrategyId::from("STRATEGY-001"),
                position_id: PositionId::from("P-001"),
                instrument_id: fixture.instrument.id(),
                account_id: AccountId::from("POLYMARKET-001"),
                opening_order_id: ClientOrderId::from("O-entry"),
                closing_order_id: None,
                entry: OrderSide::Buy,
                side: PositionSide::Long,
                signed_quantity: 0.41,
                quantity: Quantity::from("0.41"),
                last_quantity: Quantity::from("0.30"),
                last_price: Price::from("1.000"),
                currency: Currency::USDC(),
                realized_pnl: Some(Money::from("0.19400000 USDC")),
            },
            PositionEffectTrace {
                kind: PositionEffectKind::Closed,
                trader_id: TraderId::from("TRADER-001"),
                strategy_id: StrategyId::from("STRATEGY-001"),
                position_id: PositionId::from("P-001"),
                instrument_id: fixture.instrument.id(),
                account_id: AccountId::from("POLYMARKET-001"),
                opening_order_id: ClientOrderId::from("O-entry"),
                closing_order_id: Some(ClientOrderId::from("O-settlement")),
                entry: OrderSide::Buy,
                side: PositionSide::Flat,
                signed_quantity: 0.0,
                quantity: Quantity::from("0.00"),
                last_quantity: Quantity::from("0.41"),
                last_price: Price::from("1.000"),
                currency: Currency::USDC(),
                realized_pnl: Some(Money::from("0.43180000 USDC")),
            },
        ]);
        fixture.account_cash_after_fills.pop();
        fixture.account_cash_after_fills.extend([
            Money::from("1000000.02180000 USDC"),
            Money::from("1000000.43180000 USDC"),
        ]);

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("settlement must remain one synthetic fill");
        assert!(
            error
                .to_string()
                .contains("settlement must contain exactly one fill")
        );
    }

    #[test]
    fn rejects_quote_submission_without_conversion_witness() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            quote_conversion, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("fixture entry order changed")
        };
        *quote_conversion = None;
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("quote conversion must be witnessed before its fill");
        assert!(error.to_string().contains("quote conversion witness"));
    }

    #[test]
    fn rejects_quote_conversion_quantity_drift() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            quote_conversion: Some(update),
            ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("fixture quote conversion changed")
        };
        update.quantity = Quantity::from("2.72");

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("quote conversion quantity drift must fail closed");
        assert!(error.to_string().contains("conversion witness quantity"));
    }

    #[test]
    fn rejects_quote_conversion_identity_drift() {
        let mutations: Vec<OrderUpdatedMutation> = vec![
            Box::new(|update| update.trader_id = TraderId::from("OTHER-001")),
            Box::new(|update| update.strategy_id = StrategyId::from("OTHER-001")),
            Box::new(|update| update.instrument_id = InstrumentId::from("OTHER.SIM")),
            Box::new(|update| update.client_order_id = ClientOrderId::from("OTHER-001")),
        ];
        for mutate in mutations {
            let mut fixture = fixture();
            let ExecutionOrderCause::Submitted {
                quote_conversion: Some(update),
                ..
            } = &mut fixture.orders[0].cause
            else {
                panic!("fixture quote conversion changed")
            };
            mutate(update);

            let error = validate_execution_contract(&fixture.trace())
                .expect_err("quote conversion identity drift must fail closed");
            assert!(error.to_string().contains("conversion witness identity"));
        }
    }

    #[test]
    fn rejects_quote_conversion_metadata_drift() {
        let mutations: Vec<OrderUpdatedMutation> = vec![
            Box::new(|update| update.account_id = Some(AccountId::from("OTHER-001"))),
            Box::new(|update| update.price = Some(Price::from("0.421"))),
            Box::new(|update| update.trigger_price = Some(Price::from("0.421"))),
            Box::new(|update| update.protection_price = Some(Price::from("0.421"))),
            Box::new(|update| update.reconciliation = true),
        ];
        for mutate in mutations {
            let mut fixture = fixture();
            let ExecutionOrderCause::Submitted {
                quote_conversion: Some(update),
                ..
            } = &mut fixture.orders[0].cause
            else {
                panic!("fixture quote conversion changed")
            };
            mutate(update);

            validate_execution_contract(&fixture.trace())
                .expect_err("quote conversion metadata drift must fail closed");
        }
    }

    #[test]
    fn rejects_correlated_account_drift_from_configured_authority() {
        let mut fixture = fixture();
        let wrong_account = AccountId::from("OTHER-001");
        for order in &mut fixture.orders {
            if let ExecutionOrderCause::Submitted {
                submitted_order,
                quote_conversion,
                ..
            } = &mut order.cause
            {
                submitted_order.account_id = wrong_account;
                if let Some(update) = quote_conversion {
                    update.account_id = Some(wrong_account);
                }
            }
            for fill in &mut order.fills {
                fill.account_id = wrong_account;
            }
        }
        for effect in &mut fixture.position_effects {
            effect.account_id = wrong_account;
        }

        let error = validate_execution_contract(&fixture.trace()).expect_err(
            "correlated downstream drift must not override the configured account anchor",
        );
        assert!(error.to_string().contains("configured account"));
    }

    #[test]
    fn rejects_quote_conversion_that_remains_quote_denominated() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            quote_conversion: Some(update),
            ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("fixture quote conversion changed")
        };
        update.is_quote_quantity = true;

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("quote conversion must produce a base-denominated quantity");
        assert!(error.to_string().contains("retains quote"));
    }

    #[test]
    fn rejects_quote_conversion_with_venue_order_identity_before_acceptance() {
        let mut fixture = fixture();
        let venue_order_id = fixture.orders[0].fills[0].venue_order_id;
        let ExecutionOrderCause::Submitted {
            quote_conversion: Some(update),
            ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("fixture quote conversion changed")
        };
        update.venue_order_id = Some(venue_order_id);

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("conversion witness must represent the pinned pre-submission boundary");
        assert!(error.to_string().contains("venue-order identity"));
    }

    #[test]
    fn rejects_conversion_witness_for_base_denominated_submission() {
        let mut fixture = fixture();
        let conversion = quote_conversion(&fixture.orders[1].fills[0], Quantity::from("2.00"));
        let ExecutionOrderCause::Submitted {
            quote_conversion, ..
        } = &mut fixture.orders[1].cause
        else {
            panic!("fixture normal exit changed")
        };
        *quote_conversion = Some(Box::new(conversion));

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("base submission must not carry quote conversion evidence");
        assert!(error.to_string().contains("unexpected quote conversion"));
    }

    #[test]
    fn rejects_normal_fill_divergent_from_submission_book() {
        let mut fixture = fixture();
        fixture.orders[1].fills[0].last_px = Price::from("0.420");
        let error = validate_execution_contract(&fixture.trace()).expect_err("book drift");
        assert!(error.to_string().contains("executable book at submission"));
    }

    #[test]
    fn rejects_fill_side_divergent_from_submitted_order() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            submitted_order, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("entry cause changed")
        };
        submitted_order.order_side = OrderSide::Sell;

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("fill side must match its submitted order");
        assert!(error.to_string().contains("submitted order semantics"));
    }

    #[test]
    fn rejects_fill_type_divergent_from_submitted_order() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            submitted_order, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("entry cause changed")
        };
        submitted_order.order_type = OrderType::Limit;

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("fill type must match its submitted order");
        assert!(error.to_string().contains("submitted order semantics"));
    }

    #[test]
    fn rejects_fill_identity_divergent_from_submitted_order() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            submitted_order, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("entry cause changed")
        };
        submitted_order.strategy_id = StrategyId::from("OTHER-001");

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("fill identity must match its submitted order");
        assert!(error.to_string().contains("submitted order semantics"));
    }

    #[test]
    fn rejects_position_effect_causal_identity_drift() {
        type Mutation = Box<dyn Fn(&mut Fixture)>;
        let mutations: Vec<Mutation> = vec![
            Box::new(|fixture| fixture.position_effects[0].trader_id = TraderId::from("OTHER-001")),
            Box::new(|fixture| {
                fixture.position_effects[0].strategy_id = StrategyId::from("OTHER-001")
            }),
            Box::new(|fixture| {
                fixture.position_effects[0].opening_order_id = ClientOrderId::from("OTHER-001")
            }),
            Box::new(|fixture| fixture.position_effects[0].entry = OrderSide::Sell),
            Box::new(|fixture| fixture.position_effects[0].signed_quantity = 9.99),
            Box::new(|fixture| fixture.position_effects[0].currency = Currency::EUR()),
            Box::new(|fixture| {
                fixture.position_effects[2].closing_order_id =
                    Some(ClientOrderId::from("OTHER-001"))
            }),
        ];

        for mutate in mutations {
            let mut fixture = fixture();
            mutate(&mut fixture);
            let error = validate_execution_contract(&fixture.trace())
                .expect_err("position-effect causal identity drift must fail closed");
            assert!(
                error
                    .to_string()
                    .contains("position mutation does not identify its causal lifecycle")
            );
        }
    }

    #[test]
    fn rejects_client_order_identity_reused_across_lifecycle_roles() {
        let mut fixture = fixture();
        let entry_client_order_id = fixture.orders[0].fills[0].client_order_id;
        fixture.orders[1].fills[0].client_order_id = entry_client_order_id;
        let ExecutionOrderCause::Submitted {
            submitted_order, ..
        } = &mut fixture.orders[1].cause
        else {
            panic!("reduction cause changed")
        };
        submitted_order.client_order_id = entry_client_order_id;

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("lifecycle roles must use distinct client order identities");
        assert!(error.to_string().contains("identity partition"));
    }

    #[test]
    fn rejects_venue_order_identity_reused_across_lifecycle_roles() {
        let mut fixture = fixture();
        fixture.orders[1].fills[0].venue_order_id = fixture.orders[0].fills[0].venue_order_id;

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("lifecycle roles must use distinct venue order identities");
        assert!(error.to_string().contains("identity partition"));
    }

    #[test]
    fn rejects_fill_client_identity_divergent_from_submitted_order() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            submitted_order, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("entry cause changed")
        };
        submitted_order.client_order_id = ClientOrderId::from("O-OTHER");

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("fill client identity must match its submitted order");
        assert!(error.to_string().contains("submitted order semantics"));
    }

    #[test]
    fn rejects_fill_trader_identity_divergent_from_submitted_order() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            submitted_order, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("entry cause changed")
        };
        submitted_order.trader_id = TraderId::from("OTHER-001");

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("fill trader identity must match its submitted order");
        assert!(error.to_string().contains("submitted order semantics"));
    }

    #[test]
    fn rejects_post_only_market_submission() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            submitted_order, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("entry cause changed")
        };
        submitted_order.post_only = true;

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("market-taker evidence cannot be post-only");
        assert!(error.to_string().contains("submitted order semantics"));
    }

    #[test]
    fn rejects_reconciliation_submission() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            submitted_order, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("entry cause changed")
        };
        submitted_order.reconciliation = true;

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("reconciliation submissions are outside the #789 lifecycle");
        assert!(error.to_string().contains("submitted order semantics"));
    }

    #[test]
    fn rejects_reconciliation_fill() {
        let mut fixture = fixture();
        fixture.orders[0].fills[0].reconciliation = true;

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("reconciliation fills are outside the frozen #789 lifecycle");
        assert!(error.to_string().contains("market-order identity"));
    }

    #[test]
    fn rejects_embedded_submit_identity_drift() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            submitted_order, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("entry cause changed")
        };
        submitted_order.instrument_id = InstrumentId::from("OTHER.POLYMARKET");

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("embedded submit identity drift must fail closed");
        assert!(error.to_string().contains("submitted order semantics"));
    }

    #[test]
    fn rejects_executable_book_for_another_instrument() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            executable_book, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("entry cause changed")
        };
        **executable_book = one_level_book(
            nautilus_model::identifiers::InstrumentId::from("OTHER.POLYMARKET"),
            OrderSide::Sell,
            "0.420",
            "21.52",
            1,
        );
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("wrong executable-book instrument must fail closed");
        assert!(error.to_string().contains("book instrument"));
    }

    #[test]
    fn rejects_depth_over_consumption() {
        let mut fixture = fixture();
        fixture.orders[0].fills[0].last_qty = Quantity::from("21.53");
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }

    #[test]
    fn rejects_non_market_entry_at_market_only_guard() {
        let mut fixture = fixture();
        fixture.orders[0].fills[0].order_type = OrderType::Limit;
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("non-market entry must fail the market-only contract");
        assert!(error.to_string().contains("submitted order semantics"));
    }

    #[test]
    fn rejects_second_opening_effect() {
        let mut fixture = fixture();
        fixture.orders[1].fills[0].order_side = OrderSide::Buy;
        let submitted_order =
            submitted_order(&fixture.orders[1].fills[0], Quantity::from("2.00"), false);
        fixture.orders[1].cause = ExecutionOrderCause::Submitted {
            executable_book: Box::new(one_level_book(
                fixture.instrument.id(),
                OrderSide::Sell,
                "0.430",
                "2.00",
                2,
            )),
            submitted_order,
            quote_conversion: None,
        };
        fixture.position_effects[1].side = PositionSide::Long;
        fixture.position_effects[1].quantity = Quantity::from("4.71");
        fixture.position_effects[1].last_quantity = Quantity::from("2.00");
        fixture.position_effects[1].realized_pnl = None;
        fixture.account_cash_after_fills[1] = Money::from("999998.00180000 USDC");
        let error = validate_execution_contract(&fixture.trace()).expect_err("second entry");
        assert!(error.to_string().contains("entry or reducing"));
    }

    #[test]
    fn rejects_multiple_normal_reduction_orders() {
        let mut fixture = fixture();
        let instrument_id = fixture.instrument.id();
        let position_id = fixture.position_effects[0].position_id;

        let first_reduction_fill = test_fill(
            instrument_id,
            position_id,
            OrderSide::Sell,
            "normal-exit-one",
            "0.430",
            "0.50",
            2,
        );
        fixture.orders[1] = ExecutionOrderTrace {
            cause: ExecutionOrderCause::Submitted {
                executable_book: Box::new(one_level_book(
                    instrument_id,
                    OrderSide::Buy,
                    "0.430",
                    "0.50",
                    2,
                )),
                submitted_order: submitted_order(
                    &first_reduction_fill,
                    Quantity::from("0.50"),
                    false,
                ),
                quote_conversion: None,
            },
            fills: vec![first_reduction_fill],
        };
        let second_reduction_fill = test_fill(
            instrument_id,
            position_id,
            OrderSide::Sell,
            "normal-exit-two",
            "0.430",
            "1.50",
            3,
        );
        fixture.orders.insert(
            2,
            ExecutionOrderTrace {
                cause: ExecutionOrderCause::Submitted {
                    executable_book: Box::new(one_level_book(
                        instrument_id,
                        OrderSide::Buy,
                        "0.430",
                        "1.50",
                        3,
                    )),
                    submitted_order: submitted_order(
                        &second_reduction_fill,
                        Quantity::from("1.50"),
                        false,
                    ),
                    quote_conversion: None,
                },
                fills: vec![second_reduction_fill],
            },
        );
        fixture.position_effects.insert(
            1,
            PositionEffectTrace {
                kind: PositionEffectKind::Changed,
                trader_id: TraderId::from("TRADER-001"),
                strategy_id: StrategyId::from("STRATEGY-001"),
                position_id,
                instrument_id,
                account_id: AccountId::from("POLYMARKET-001"),
                opening_order_id: ClientOrderId::from("O-entry"),
                closing_order_id: None,
                entry: OrderSide::Buy,
                side: PositionSide::Long,
                signed_quantity: 2.21,
                quantity: Quantity::from("2.21"),
                last_quantity: Quantity::from("0.50"),
                last_price: Price::from("0.430"),
                currency: Currency::USDC(),
                realized_pnl: Some(Money::from("0.00500000 USDC")),
            },
        );
        fixture.position_effects[2].last_quantity = Quantity::from("1.50");
        fixture
            .account_cash_after_fills
            .insert(1, Money::from("999999.07680000 USDC"));

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("#789 must reject a second normal reduction order");
        assert!(error.to_string().contains("single normal reduction"));
    }

    #[test]
    fn rejects_normal_exit_reversal() {
        let mut fixture = fixture();
        if let ExecutionOrderCause::Submitted {
            executable_book,
            submitted_order,
            ..
        } = &mut fixture.orders[1].cause
        {
            **executable_book =
                one_level_book(fixture.instrument.id(), OrderSide::Buy, "0.430", "3.00", 2);
            submitted_order.quantity = Quantity::from("3.00");
        }
        fixture.orders[1].fills[0].last_qty = Quantity::from("3.00");
        let error = validate_execution_contract(&fixture.trace()).expect_err("reversal");
        assert!(error.to_string().contains("reverses or reopens"));
    }

    #[test]
    fn rejects_dropped_or_duplicated_cash_leg() {
        let fixture = fixture();
        let mut trace = fixture.trace();
        trace.terminal_cash = trace
            .terminal_cash
            .checked_add(trace.realized_pnl)
            .expect("duplicated fixture cash leg should add exactly");
        assert!(validate_execution_contract(&trace).is_err());
    }

    #[test]
    fn rejects_intermediate_account_cash_drift() {
        let mut fixture = fixture();
        fixture.account_cash_after_fills[1] = Money::from("999999.73180000 USDC");
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("intermediate AccountState drift must fail closed");
        assert!(error.to_string().contains("per-fill cash fold"));
    }

    #[test]
    fn rejects_correlated_dropped_cash_and_pnl_legs() {
        let fixture = fixture();
        let mut trace = fixture.trace();
        trace.terminal_cash = trace.initial_cash;
        trace.realized_pnl = Money::from("0.00 USDC");
        assert!(validate_execution_contract(&trace).is_err());
    }

    #[test]
    fn rejects_terminal_fill_price_divergent_from_settlement() {
        let mut fixture = fixture();
        fixture.orders[2].fills[0].last_px = Price::from("0.500");
        fixture.position_effects[2].last_price = Price::from("0.500");
        fixture.position_effects[2].realized_pnl = Some(Money::from("0.07680000 USDC"));
        fixture.account_cash_after_fills[2] = Money::from("1000000.07680000 USDC");
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("terminal fill price must be bound to configured settlement");
        assert!(error.to_string().contains("declared close price"));
    }

    #[test]
    fn rejects_incomplete_terminal_close_at_terminal_quantity_guard() {
        let mut fixture = fixture();
        fixture.orders[2].fills[0].last_qty = Quantity::from("0.70");
        fixture.position_effects[2].kind = PositionEffectKind::Changed;
        fixture.position_effects[2].side = PositionSide::Long;
        fixture.position_effects[2].quantity = Quantity::from("0.01");
        fixture.position_effects[2].last_quantity = Quantity::from("0.70");
        fixture.position_effects[2].realized_pnl = Some(Money::from("0.42600000 USDC"));
        fixture.account_cash_after_fills[2] = Money::from("1000000.42180000 USDC");
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("incomplete terminal close must fail closed");
        assert!(error.to_string().contains("exactly close the remainder"));
    }

    #[test]
    fn rejects_missing_settlement() {
        let mut fixture = fixture();
        fixture.orders.pop();
        let error = validate_execution_contract(&fixture.trace()).expect_err("missing settlement");
        assert!(error.to_string().contains("entry, normal exit"));
    }

    #[test]
    fn rejects_duplicate_trade_id() {
        let mut fixture = fixture();
        fixture.orders[1].fills[0].trade_id = fixture.orders[0].fills[0].trade_id;
        let error = validate_execution_contract(&fixture.trace()).expect_err("duplicate trade id");
        assert!(error.to_string().contains("duplicate trade ID"));
    }

    #[test]
    fn rejects_wrong_commission() {
        let fixture = fixture();
        let mut trace = fixture.trace();
        trace.position_commissions = vec![Money::from("0.01 USDC")];
        assert!(validate_execution_contract(&trace).is_err());
    }

    #[test]
    fn rejects_terminal_commission_in_another_currency() {
        let mut fixture = fixture();
        fixture.position_commissions = vec![Money::from("0.01000000 BTC")];
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("non-zero terminal commission in another currency must fail closed");
        assert!(error.to_string().contains("commission map"));
    }

    #[test]
    fn rejects_correlated_wrong_fill_and_position_commission() {
        let mut fixture = fixture();
        let commission = Money::from("0.01000000 USDC");
        fixture.orders[0].fills[0].commission = Some(commission);
        fixture.position_commissions = vec![commission];
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("correlated non-zero commission must violate the zero-fee assumption");
        assert!(error.to_string().contains("zero taker fee"));
    }

    #[test]
    fn rejects_missing_fill_commission_evidence() {
        let mut fixture = fixture();
        fixture.orders[0].fills[0].commission = None;
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("missing per-fill commission must fail closed");
        assert!(error.to_string().contains("commission evidence"));
    }

    #[test]
    fn rejects_missing_terminal_commission_currency() {
        let mut fixture = fixture();
        fixture.position_commissions.clear();
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("missing terminal commission currency must fail closed");
        assert!(error.to_string().contains("commission map"));
    }

    #[test]
    fn rejects_extra_zero_terminal_commission_currency() {
        let mut fixture = fixture();
        fixture
            .position_commissions
            .push(Money::from("0.00000000 BTC"));
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("extra zero-valued terminal currency must fail closed");
        assert!(error.to_string().contains("commission map"));
    }

    #[test]
    fn rejects_fill_price_with_wrong_raw_precision() {
        let mut fixture = fixture();
        fixture.orders[0].fills[0].last_px = Price::from("0.4200");
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("fill price precision must be instrument-derived");
        assert!(error.to_string().contains("price precision"));
    }

    #[test]
    fn rejects_fill_quantity_with_correct_precision_but_wrong_increment() {
        let mut fixture = fixture();
        let InstrumentAny::BinaryOption(instrument) = &mut fixture.instrument else {
            panic!("fixture instrument changed")
        };
        instrument.size_increment = Quantity::from("0.02");

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("correct precision cannot bypass the size increment");
        assert!(error.to_string().contains("size precision"));
    }

    #[test]
    fn rejects_fill_price_with_correct_precision_but_wrong_increment() {
        let mut fixture = fixture();
        let InstrumentAny::BinaryOption(instrument) = &mut fixture.instrument else {
            panic!("fixture instrument changed")
        };
        instrument.price_increment = Price::from("0.004");

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("correct precision cannot bypass the price increment");
        assert!(error.to_string().contains("price precision"));
    }

    #[test]
    fn rejects_position_effect_price_with_wrong_raw_precision() {
        let mut fixture = fixture();
        fixture.position_effects[0].last_price = Price::from("0.4200");
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("position price precision must be instrument-derived");
        assert!(error.to_string().contains("position mutation price"));
    }

    #[test]
    fn rejects_position_effect_quantity_with_wrong_raw_precision() {
        let mut fixture = fixture();
        fixture.position_effects[0].quantity = Quantity::from("2.710");
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("position quantity precision must be instrument-derived");
        assert!(error.to_string().contains("position mutation quantity"));
    }

    #[test]
    fn rejects_quote_submission_with_wrong_raw_quantity_precision() {
        let mut fixture = fixture();
        let ExecutionOrderCause::Submitted {
            submitted_order, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("fixture entry order changed")
        };
        submitted_order.quantity = Quantity::from("1.140");

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("quote submission precision must be instrument-derived");
        assert!(error.to_string().contains("submitted quantity precision"));
    }

    #[test]
    fn rejects_zero_quote_conversion_price_without_panicking() {
        let mut fixture = fixture();
        let instrument_id = fixture.instrument.id();
        let ExecutionOrderCause::Submitted {
            executable_book, ..
        } = &mut fixture.orders[0].cause
        else {
            panic!("fixture entry order changed")
        };
        **executable_book = one_level_book(instrument_id, OrderSide::Sell, "0.000", "21.52", 1);

        let error = validate_execution_contract(&fixture.trace())
            .expect_err("zero quote conversion price must fail with a typed error");
        assert!(error.to_string().contains("strictly positive"));
    }

    #[test]
    fn rejects_insufficient_executable_depth_for_a_filled_normal_order() {
        let fixture = fixture();
        let book = one_level_book(fixture.instrument.id(), OrderSide::Sell, "0.420", "2.00", 1);

        let error = independent_market_sweep(&book, OrderSide::Buy, Quantity::from("2.71"))
            .expect_err("a fully filled normal order requires enough executable depth");
        assert!(error.to_string().contains("insufficient executable depth"));
    }

    #[test]
    fn accepts_instrument_size_precision_above_two_decimals() {
        let mut fixture = fixture();
        let InstrumentAny::BinaryOption(instrument) = &mut fixture.instrument else {
            panic!("fixture instrument changed")
        };
        instrument.size_precision = 3;
        instrument.size_increment = Quantity::from("0.001");
        if let ExecutionOrderCause::Submitted {
            submitted_order,
            quote_conversion,
            ..
        } = &mut fixture.orders[0].cause
        {
            submitted_order.quantity = Quantity::from("2.714");
            submitted_order.quote_quantity = false;
            *quote_conversion = None;
        }
        fixture.orders[0].fills[0].last_qty = Quantity::from("2.714");
        fixture.position_effects[0].quantity = Quantity::from("2.714");
        fixture.position_effects[0].last_quantity = Quantity::from("2.714");
        if let ExecutionOrderCause::Submitted {
            executable_book,
            submitted_order,
            ..
        } = &mut fixture.orders[1].cause
        {
            **executable_book =
                one_level_book(fixture.instrument.id(), OrderSide::Buy, "0.430", "2.000", 2);
            submitted_order.quantity = Quantity::from("2.000");
        }
        fixture.orders[1].fills[0].last_qty = Quantity::from("2.000");
        fixture.position_effects[1].quantity = Quantity::from("0.714");
        fixture.position_effects[1].last_quantity = Quantity::from("2.000");
        fixture.orders[2].fills[0].last_qty = Quantity::from("0.714");
        fixture.position_effects[2].quantity = Quantity::from("0.000");
        fixture.position_effects[2].last_quantity = Quantity::from("0.714");
        fixture.realized_pnl = Money::from("0.43412000 USDC");
        fixture.position_effects[2].realized_pnl = Some(fixture.realized_pnl);
        fixture.terminal_cash = fixture
            .initial_cash
            .checked_add(fixture.realized_pnl)
            .expect("precision fixture cash");
        fixture.account_cash_after_fills = vec![
            Money::from("999998.86012000 USDC"),
            Money::from("999999.72012000 USDC"),
            fixture.terminal_cash,
        ];
        validate_execution_contract(&fixture.trace()).expect("instrument-derived precision");
    }

    #[test]
    fn rejects_canonical_config_bytes_hash_integrity_mismatch() {
        let mut fixture = fixture();
        fixture.config_bytes.push(b' ');
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }

    #[test]
    fn rejects_position_mutation_quantity_drift() {
        let mut fixture = fixture();
        fixture.position_effects[1].quantity = Quantity::from("0.72");
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("position mutation drift must fail closed");
        assert!(error.to_string().contains("independently folded exposure"));
    }
}
