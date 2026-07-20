//! Exact economic validation for opted-in historical backtest diagnostics.
//!
//! This module does not model execution. It compares the runner's observed
//! economic trace with results produced by NautilusTrader's shared order-book,
//! instrument sizing, position, and account primitives.
//!
//! Configuration provenance is an integrity agreement between canonical
//! resolved bytes and their recorded hash. It is not a frozen configuration
//! golden; applied-override sensitivity is verified by the configuration owner.

use anyhow::{Context, Result, ensure};
use nautilus_core::UnixNanos;
use nautilus_model::{
    data::BookOrder,
    enums::{OrderSide, OrderType},
    events::OrderFilled,
    instruments::{Instrument, InstrumentAny},
    orderbook::OrderBook,
    position::Position,
    types::{Money, Price, Quantity, fixed::FIXED_PRECISION},
};

use crate::hashing::sha256_hex;

pub struct ExecutionOrderTrace {
    pub submission_timestamp: Option<UnixNanos>,
    pub executable_book: Option<OrderBook>,
    pub submitted_quantity: Quantity,
    pub quote_quantity: bool,
    pub effective_base_quantity: Quantity,
    pub fills: Vec<OrderFilled>,
}

pub struct ExecutionContractTrace<'a> {
    pub instrument: &'a InstrumentAny,
    pub orders: &'a [ExecutionOrderTrace],
    pub position_fills: &'a [OrderFilled],
    pub settlement_price: Price,
    pub initial_cash: Money,
    pub terminal_cash: Money,
    pub realized_pnl: Money,
    pub position_commission: Money,
    pub expected_fill_commission: Money,
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
        !trace.orders.is_empty(),
        "execution contract requires at least one order"
    );
    let opening_fill = trace
        .position_fills
        .first()
        .context("execution contract requires position fills")?;
    let position_id = opening_fill
        .position_id
        .context("opening fill has no position ID")?;
    ensure!(
        trace.position_fills.iter().all(|fill| {
            fill.instrument_id == trace.instrument.id() && fill.position_id == Some(position_id)
        }),
        "position replay fills do not share one instrument and position ID"
    );

    let order_fill_count: usize = trace.orders.iter().map(|order| order.fills.len()).sum();
    ensure!(
        order_fill_count == trace.position_fills.len()
            && trace
                .orders
                .iter()
                .flat_map(|order| &order.fills)
                .zip(trace.position_fills)
                .all(|(order_fill, position_fill)| order_fill == position_fill),
        "ordered order fills do not exactly equal ordered position fills"
    );

    let size_precision = trace.instrument.size_precision();
    let mut opening_side = None;
    let mut remaining_quantity = Quantity::zero(size_precision);
    let mut entry_fill_count = 0;
    let mut normal_exit_fill_count = 0;
    let mut settlement_fill_count = 0;

    for (order_index, order) in trace.orders.iter().enumerate() {
        let first_fill = order
            .fills
            .first()
            .with_context(|| format!("execution order {order_index} has no fills"))?;
        let order_side = first_fill.order_side;
        ensure!(
            order_side != OrderSide::NoOrderSide
                && order.fills.iter().all(|fill| {
                    fill.instrument_id == trace.instrument.id()
                        && fill.position_id == Some(position_id)
                        && fill.client_order_id == first_fill.client_order_id
                        && fill.order_side == order_side
                        && fill.order_type == OrderType::Market
                        && fill.last_qty.precision == size_precision
                }),
            "execution order fills must be one-instrument, one-position, one-side market fills at instrument size precision"
        );
        ensure!(
            order.effective_base_quantity.precision == size_precision,
            "effective base quantity precision does not equal instrument size precision"
        );
        let order_filled_quantity =
            order
                .fills
                .iter()
                .try_fold(Quantity::zero(size_precision), |total, fill| {
                    total
                        .checked_add(fill.last_qty)
                        .context("order fill quantity addition overflow or precision mismatch")
                })?;

        match (order.submission_timestamp, order.executable_book.as_ref()) {
            (Some(submission_timestamp), Some(executable_book)) => {
                ensure!(
                    settlement_fill_count == 0,
                    "submitted normal order appears after settlement"
                );
                ensure!(
                    order
                        .fills
                        .iter()
                        .all(|fill| fill.ts_event >= submission_timestamp),
                    "normal order fill predates its submission timestamp"
                );
                let opposing_best_price = match order_side {
                    OrderSide::Buy => executable_book.best_ask_price(),
                    OrderSide::Sell => executable_book.best_bid_price(),
                    OrderSide::NoOrderSide => None,
                }
                .context("executable book has no opposing price")?;
                if order.quote_quantity {
                    let expected_base_quantity = trace
                        .instrument
                        .get_base_quantity(order.submitted_quantity, opposing_best_price);
                    ensure!(
                        order.effective_base_quantity == expected_base_quantity,
                        "quote/base conversion mismatch: submitted {} at {} resolves to {}, observed {}",
                        order.submitted_quantity,
                        opposing_best_price,
                        expected_base_quantity,
                        order.effective_base_quantity,
                    );
                } else {
                    ensure!(
                        order.effective_base_quantity == order.submitted_quantity,
                        "base-denominated order quantity changed before execution"
                    );
                }
                let market_price = match order_side {
                    OrderSide::Buy => Some(Price::max(FIXED_PRECISION)),
                    OrderSide::Sell => Some(Price::min(FIXED_PRECISION)),
                    OrderSide::NoOrderSide => None,
                }
                .context("market order has no specified side")?;
                let expected_fills = executable_book.simulate_fills(&BookOrder::new(
                    order_side,
                    market_price,
                    order.effective_base_quantity,
                    0,
                ));
                ensure!(
                    expected_fills.len() == order.fills.len()
                        && expected_fills.iter().zip(&order.fills).all(
                            |((expected_price, expected_quantity), observed)| {
                                *expected_price == observed.last_px
                                    && *expected_quantity == observed.last_qty
                            }
                        ),
                    "observed normal-order fills do not equal deterministic fills from the executable book"
                );

                match opening_side {
                    None => {
                        opening_side = Some(order_side);
                        remaining_quantity = remaining_quantity
                            .checked_add(order_filled_quantity)
                            .context("entry fill quantity overflow or precision mismatch")?;
                        entry_fill_count += order.fills.len();
                    }
                    Some(side) if side == order_side => {
                        anyhow::bail!(
                            "complete lifecycle permits exactly one opening order; additional opening effect detected"
                        )
                    }
                    Some(_) => {
                        remaining_quantity = remaining_quantity
                            .checked_sub(order_filled_quantity)
                            .context(
                                "normal exit over-closes position or has precision mismatch",
                            )?;
                        normal_exit_fill_count += order.fills.len();
                    }
                }
            }
            (None, None) => {
                ensure!(
                    order_index + 1 == trace.orders.len() && settlement_fill_count == 0,
                    "settlement must be the single final lifecycle order"
                );
                let entry_side = opening_side.context("settlement appears before an entry")?;
                ensure!(
                    order_side != entry_side,
                    "settlement fill does not have a closing position effect"
                );
                ensure!(
                    !remaining_quantity.is_zero()
                        && order_filled_quantity == remaining_quantity
                        && order.submitted_quantity == remaining_quantity
                        && order.effective_base_quantity == remaining_quantity,
                    "settlement does not exactly close the remaining instrument-precision quantity"
                );
                ensure!(
                    !order.quote_quantity
                        && order
                            .fills
                            .iter()
                            .all(|fill| fill.last_px == trace.settlement_price),
                    "settlement fill does not use the configured settlement price and base quantity"
                );
                remaining_quantity = remaining_quantity
                    .checked_sub(order_filled_quantity)
                    .context("settlement quantity subtraction failed")?;
                settlement_fill_count = order.fills.len();
            }
            _ => anyhow::bail!(
                "normal orders require both submission timestamp and executable book; settlement requires neither"
            ),
        }
    }

    ensure!(
        entry_fill_count > 0 && settlement_fill_count > 0 && remaining_quantity.is_zero(),
        "complete lifecycle must contain an entry and exact terminal settlement close"
    );

    let mut replayed_position = Position::new(trace.instrument, opening_fill.clone());
    for fill in &trace.position_fills[1..] {
        replayed_position.apply(fill);
    }
    let replayed_pnl = replayed_position
        .realized_pnl
        .context("replayed position did not realize PnL")?;
    ensure!(
        replayed_pnl == trace.realized_pnl,
        "cached realized PnL does not equal PnL replayed from typed fills"
    );

    let cash_change = trace
        .terminal_cash
        .checked_sub(trace.initial_cash)
        .context("terminal cash subtraction overflow or scale mismatch")?;
    ensure!(
        cash_change == replayed_pnl,
        "terminal cash change does not equal PnL replayed from typed fills"
    );

    let total_fill_commission = trace.position_fills.iter().try_fold(
        Money::zero(trace.position_commission.currency),
        |total, fill| {
            let commission = fill
                .commission
                .unwrap_or_else(|| Money::zero(trace.position_commission.currency));
            ensure!(
                commission == trace.expected_fill_commission,
                "fill commission does not equal the explicit fixture assumption"
            );
            total
                .checked_add(commission)
                .context("fill commission addition overflow or scale mismatch")
        },
    )?;
    ensure!(
        total_fill_commission == trace.position_commission,
        "position commission does not equal the sum of fill commissions"
    );

    Ok(ExecutionContractReport {
        validated_fill_count: trace.position_fills.len(),
        entry_fill_count,
        normal_exit_fill_count,
        settlement_fill_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_core::UnixNanos;
    use nautilus_model::{enums::BookType, instruments::stubs};

    struct Fixture {
        instrument: InstrumentAny,
        orders: Vec<ExecutionOrderTrace>,
        position_fills: Vec<OrderFilled>,
        initial_cash: Money,
        terminal_cash: Money,
        realized_pnl: Money,
        position_commission: Money,
        config_bytes: Vec<u8>,
        config_hash: String,
    }

    impl Fixture {
        fn trace(&self) -> ExecutionContractTrace<'_> {
            ExecutionContractTrace {
                instrument: &self.instrument,
                orders: &self.orders,
                position_fills: &self.position_fills,
                settlement_price: Price::from("1.000"),
                initial_cash: self.initial_cash,
                terminal_cash: self.terminal_cash,
                realized_pnl: self.realized_pnl,
                position_commission: self.position_commission,
                expected_fill_commission: Money::from("0.00 USDC"),
                canonical_resolved_config_bytes: &self.config_bytes,
                canonical_resolved_config_sha256: &self.config_hash,
            }
        }
    }

    fn fixture() -> Fixture {
        let instrument = InstrumentAny::BinaryOption(stubs::binary_option());
        let mut entry_book = OrderBook::new(instrument.id(), BookType::L2_MBP);
        entry_book.add(
            BookOrder::new(
                OrderSide::Sell,
                Price::from("0.420"),
                Quantity::from("21.52"),
                1,
            ),
            0,
            1,
            UnixNanos::from(1),
        );
        let config_bytes = br#"{"order_type":"MARKET","quote_quantity":true}"#.to_vec();
        let config_hash = sha256_hex(&config_bytes);
        let position_id = nautilus_model::identifiers::PositionId::from("P-001");
        let entry_fill = test_fill(
            instrument.id(),
            position_id,
            OrderSide::Buy,
            "entry",
            "0.420",
        );
        let exit_fill = test_fill(
            instrument.id(),
            position_id,
            OrderSide::Sell,
            "exit",
            "1.000",
        );
        let mut position = Position::new(&instrument, entry_fill.clone());
        position.apply(&exit_fill);
        let realized_pnl = position
            .realized_pnl
            .expect("fixture position should realize PnL");
        let initial_cash = Money::from("1000000.00 USDC");
        let terminal_cash = initial_cash
            .checked_add(realized_pnl)
            .expect("fixture cash addition should be exact");
        Fixture {
            instrument,
            orders: vec![
                ExecutionOrderTrace {
                    submission_timestamp: Some(UnixNanos::from(1)),
                    executable_book: Some(entry_book),
                    submitted_quantity: Quantity::from("1.14"),
                    quote_quantity: true,
                    effective_base_quantity: Quantity::from("2.71"),
                    fills: vec![entry_fill.clone()],
                },
                ExecutionOrderTrace {
                    submission_timestamp: None,
                    executable_book: None,
                    submitted_quantity: Quantity::from("2.71"),
                    quote_quantity: false,
                    effective_base_quantity: Quantity::from("2.71"),
                    fills: vec![exit_fill.clone()],
                },
            ],
            position_fills: vec![entry_fill, exit_fill],
            initial_cash,
            terminal_cash,
            realized_pnl,
            position_commission: Money::from("0.00 USDC"),
            config_bytes,
            config_hash,
        }
    }

    fn reconcile_position_accounting(fixture: &mut Fixture) {
        let mut position = Position::new(&fixture.instrument, fixture.position_fills[0].clone());
        for fill in &fixture.position_fills[1..] {
            position.apply(fill);
        }
        fixture.realized_pnl = position
            .realized_pnl
            .expect("mutated fixture position should realize PnL");
        fixture.terminal_cash = fixture
            .initial_cash
            .checked_add(fixture.realized_pnl)
            .expect("mutated fixture cash should reconcile exactly");
    }

    fn sync_order_fills_from_position(fixture: &mut Fixture) {
        let mut position_fills = fixture.position_fills.iter();
        for order in &mut fixture.orders {
            for order_fill in &mut order.fills {
                *order_fill = position_fills
                    .next()
                    .expect("fixture order has more fills than position")
                    .clone();
            }
        }
        assert!(
            position_fills.next().is_none(),
            "fixture position has more fills than orders"
        );
    }

    fn test_fill(
        instrument_id: nautilus_model::identifiers::InstrumentId,
        position_id: nautilus_model::identifiers::PositionId,
        side: OrderSide,
        trade_id: &str,
        price: &str,
    ) -> OrderFilled {
        use nautilus_model::{
            enums::{LiquiditySide, OrderType},
            identifiers::{AccountId, ClientOrderId, StrategyId, TradeId, TraderId, VenueOrderId},
            types::Currency,
        };

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
            Quantity::from("2.71"),
            Price::from(price),
            Currency::USDC(),
            LiquiditySide::Taker,
            nautilus_core::UUID4::new(),
            UnixNanos::from(if side == OrderSide::Buy { 1 } else { 2 }),
            UnixNanos::from(if side == OrderSide::Buy { 1 } else { 2 }),
            false,
            Some(position_id),
            None,
            None,
        )
    }

    fn complete_lifecycle_fixture() -> Fixture {
        let mut fixture = fixture();
        let mut exit_book = OrderBook::new(fixture.instrument.id(), BookType::L2_MBP);
        exit_book.add(
            BookOrder::new(
                OrderSide::Buy,
                Price::from("0.430"),
                Quantity::from("2.00"),
                2,
            ),
            0,
            2,
            UnixNanos::from(2),
        );

        fixture.position_fills[1].last_px = Price::from("0.430");
        fixture.position_fills[1].last_qty = Quantity::from("2.00");
        fixture.position_fills[1].trade_id =
            nautilus_model::identifiers::TradeId::from("normal-exit");
        fixture.position_fills[1].client_order_id =
            nautilus_model::identifiers::ClientOrderId::from("O-normal-exit");
        let mut settlement_fill = fixture.position_fills[1].clone();
        settlement_fill.last_px = Price::from("1.000");
        settlement_fill.last_qty = Quantity::from("0.71");
        settlement_fill.trade_id = nautilus_model::identifiers::TradeId::from("settlement");
        settlement_fill.client_order_id =
            nautilus_model::identifiers::ClientOrderId::from("O-settlement");
        settlement_fill.ts_event = UnixNanos::from(3);
        settlement_fill.ts_init = UnixNanos::from(3);
        fixture.position_fills.push(settlement_fill);
        reconcile_position_accounting(&mut fixture);
        fixture.orders = vec![
            ExecutionOrderTrace {
                submission_timestamp: Some(UnixNanos::from(1)),
                executable_book: fixture.orders[0].executable_book.take(),
                submitted_quantity: Quantity::from("1.14"),
                quote_quantity: true,
                effective_base_quantity: Quantity::from("2.71"),
                fills: fixture.position_fills[0..1].to_vec(),
            },
            ExecutionOrderTrace {
                submission_timestamp: Some(UnixNanos::from(2)),
                executable_book: Some(exit_book),
                submitted_quantity: Quantity::from("2.00"),
                quote_quantity: false,
                effective_base_quantity: Quantity::from("2.00"),
                fills: fixture.position_fills[1..2].to_vec(),
            },
            ExecutionOrderTrace {
                submission_timestamp: None,
                executable_book: None,
                submitted_quantity: Quantity::from("0.71"),
                quote_quantity: false,
                effective_base_quantity: Quantity::from("0.71"),
                fills: fixture.position_fills[2..3].to_vec(),
            },
        ];
        fixture
    }

    #[test]
    fn accepts_exact_shared_primitive_trace() {
        validate_execution_contract(&fixture().trace()).expect("valid trace should pass");
    }

    #[test]
    fn accepts_entry_normal_exit_and_exact_remaining_settlement() {
        let report = validate_execution_contract(&complete_lifecycle_fixture().trace())
            .expect("entry, submitted normal exit, and exact remaining settlement should pass");
        assert_eq!(report.entry_fill_count, 1);
        assert_eq!(report.normal_exit_fill_count, 1);
        assert_eq!(report.settlement_fill_count, 1);
    }

    #[test]
    fn accepts_instrument_size_precision_above_two_decimals() {
        let mut fixture = fixture();
        let InstrumentAny::BinaryOption(instrument) = &mut fixture.instrument else {
            panic!("fixture must use a binary option");
        };
        instrument.size_precision = 3;
        instrument.size_increment = Quantity::from("0.001");
        fixture.position_fills[0].last_qty = Quantity::from("2.714");
        fixture.position_fills[1].last_qty = Quantity::from("2.714");
        fixture.orders[0].effective_base_quantity = Quantity::from("2.714");
        fixture.orders[1].submitted_quantity = Quantity::from("2.714");
        fixture.orders[1].effective_base_quantity = Quantity::from("2.714");
        sync_order_fills_from_position(&mut fixture);
        reconcile_position_accounting(&mut fixture);

        validate_execution_contract(&fixture.trace())
            .expect("quantity precision must come from the instrument");
    }

    #[test]
    fn rejects_quantity_precision_not_equal_to_instrument() {
        let mut fixture = fixture();
        fixture.position_fills[0].last_qty = Quantity::from("2.710");
        fixture.position_fills[1].last_qty = Quantity::from("2.710");
        fixture.orders[0].effective_base_quantity = Quantity::from("2.710");
        fixture.orders[1].submitted_quantity = Quantity::from("2.710");
        fixture.orders[1].effective_base_quantity = Quantity::from("2.710");
        sync_order_fills_from_position(&mut fixture);
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("quantity precision must exactly equal instrument precision");
        assert!(error.to_string().contains("instrument size precision"));
    }

    #[test]
    fn rejects_normal_exit_without_submission_time_book() {
        let mut fixture = complete_lifecycle_fixture();
        fixture.orders[1].executable_book = None;
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("every submitted normal fill needs its executable book");
        assert!(
            error
                .to_string()
                .contains("require both submission timestamp")
        );
    }

    #[test]
    fn rejects_normal_exit_fill_divergent_from_submission_time_book() {
        let mut fixture = complete_lifecycle_fixture();
        fixture.position_fills[1].last_px = Price::from("0.420");
        sync_order_fills_from_position(&mut fixture);
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("normal exit fills must match their submission-time book");
        assert!(
            error
                .to_string()
                .contains("deterministic fills from the executable book")
        );
    }

    #[test]
    fn rejects_second_opening_order_by_position_effect() {
        let mut fixture = complete_lifecycle_fixture();
        let mut opening_book = OrderBook::new(fixture.instrument.id(), BookType::L2_MBP);
        opening_book.add(
            BookOrder::new(
                OrderSide::Sell,
                Price::from("0.430"),
                Quantity::from("2.00"),
                2,
            ),
            0,
            2,
            UnixNanos::from(2),
        );
        fixture.orders[1].executable_book = Some(opening_book);
        fixture.position_fills[1].order_side = OrderSide::Buy;
        sync_order_fills_from_position(&mut fixture);
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("same-side submitted fill must not be relabeled as an exit");
        assert!(error.to_string().contains("additional opening effect"));
    }

    #[test]
    fn rejects_normal_exit_that_flips_the_position() {
        let mut fixture = complete_lifecycle_fixture();
        let mut exit_book = OrderBook::new(fixture.instrument.id(), BookType::L2_MBP);
        exit_book.add(
            BookOrder::new(
                OrderSide::Buy,
                Price::from("0.430"),
                Quantity::from("3.00"),
                2,
            ),
            0,
            2,
            UnixNanos::from(2),
        );
        fixture.orders[1].executable_book = Some(exit_book);
        fixture.orders[1].submitted_quantity = Quantity::from("3.00");
        fixture.orders[1].effective_base_quantity = Quantity::from("3.00");
        fixture.position_fills[1].last_qty = Quantity::from("3.00");
        sync_order_fills_from_position(&mut fixture);
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("normal exit must not flip the position");
        assert!(
            error
                .to_string()
                .contains("normal exit over-closes position")
        );
    }

    #[test]
    fn rejects_settlement_with_opening_side() {
        let mut fixture = complete_lifecycle_fixture();
        fixture.position_fills[2].order_side = OrderSide::Buy;
        sync_order_fills_from_position(&mut fixture);
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("settlement must close by position effect");
        assert!(error.to_string().contains("closing position effect"));
    }

    #[test]
    fn rejects_lifecycle_without_settlement() {
        let mut fixture = complete_lifecycle_fixture();
        fixture.orders.pop();
        fixture.position_fills.pop();
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("complete lifecycle must end in settlement");
        assert!(
            error
                .to_string()
                .contains("exact terminal settlement close")
        );
    }

    #[test]
    fn rejects_fill_price_improvement() {
        let mut fixture = fixture();
        fixture.position_fills[0].last_px = Price::from("0.410");
        sync_order_fills_from_position(&mut fixture);
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }

    #[test]
    fn rejects_depth_over_consumption() {
        let mut fixture = fixture();
        fixture.position_fills[0].last_qty = Quantity::from("21.53");
        sync_order_fills_from_position(&mut fixture);
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }

    #[test]
    fn rejects_broken_quote_base_conversion() {
        let mut fixture = fixture();
        fixture.orders[0].effective_base_quantity = Quantity::from("2.72");
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }

    #[test]
    fn rejects_non_market_entry_at_market_only_guard() {
        let mut fixture = fixture();
        fixture.position_fills[0].order_type = OrderType::Limit;
        sync_order_fills_from_position(&mut fixture);
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("non-market entry must fail the market-only contract");
        assert!(error.to_string().contains("market fills"));
    }

    #[test]
    fn rejects_observed_last_fill_as_artificial_market_limit() {
        let mut fixture = fixture();
        let mut book = OrderBook::new(fixture.instrument.id(), BookType::L2_MBP);
        for (order_id, price, quantity) in [(1, "0.420", "1.00"), (2, "0.500", "1.71")] {
            book.add(
                BookOrder::new(
                    OrderSide::Sell,
                    Price::from(price),
                    Quantity::from(quantity),
                    order_id,
                ),
                0,
                order_id,
                UnixNanos::from(1),
            );
        }
        fixture.orders[0].executable_book = Some(book);
        fixture.position_fills[0].last_qty = Quantity::from("1.00");
        fixture.position_fills[1].last_qty = Quantity::from("1.00");
        fixture.orders[1].submitted_quantity = Quantity::from("1.00");
        fixture.orders[1].effective_base_quantity = Quantity::from("1.00");
        sync_order_fills_from_position(&mut fixture);
        reconcile_position_accounting(&mut fixture);
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("market simulation must not stop at the observed last fill price");
        assert!(
            error
                .to_string()
                .contains("deterministic fills from the executable book")
        );
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
    fn rejects_correlated_dropped_cash_and_pnl_legs() {
        let fixture = fixture();
        let mut trace = fixture.trace();
        trace.terminal_cash = trace.initial_cash;
        trace.realized_pnl = Money::from("0.00 USDC");
        assert!(validate_execution_contract(&trace).is_err());
    }

    #[test]
    fn rejects_order_position_fill_divergence() {
        let mut fixture = fixture();
        fixture.position_fills[0].last_qty = Quantity::from("1.71");
        fixture.position_fills[1].last_qty = Quantity::from("1.71");
        let mut position = Position::new(&fixture.instrument, fixture.position_fills[0].clone());
        position.apply(&fixture.position_fills[1]);
        fixture.realized_pnl = position
            .realized_pnl
            .expect("divergent fixture position should realize PnL");
        fixture.terminal_cash = fixture
            .initial_cash
            .checked_add(fixture.realized_pnl)
            .expect("divergent fixture cash should reconcile exactly");
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }

    #[test]
    fn rejects_extra_position_entry_leg_with_consistent_cash_and_pnl() {
        let mut fixture = fixture();
        let mut extra_entry = fixture.position_fills[0].clone();
        extra_entry.trade_id = nautilus_model::identifiers::TradeId::from("extra-entry");
        fixture.position_fills.insert(1, extra_entry);
        fixture.position_fills[2].last_qty = Quantity::from("5.42");
        let mut position = Position::new(&fixture.instrument, fixture.position_fills[0].clone());
        for fill in &fixture.position_fills[1..] {
            position.apply(fill);
        }
        fixture.realized_pnl = position
            .realized_pnl
            .expect("duplicated-entry fixture should realize PnL");
        fixture.terminal_cash = fixture
            .initial_cash
            .checked_add(fixture.realized_pnl)
            .expect("duplicated-entry fixture cash should reconcile exactly");
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }

    #[test]
    fn rejects_terminal_fill_price_divergent_from_settlement() {
        let mut fixture = fixture();
        fixture.position_fills[1].last_px = Price::from("0.500");
        sync_order_fills_from_position(&mut fixture);
        reconcile_position_accounting(&mut fixture);
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("terminal fill price must be bound to configured settlement");
        assert!(error.to_string().contains("configured settlement price"));
    }

    #[test]
    fn rejects_incomplete_terminal_close_at_terminal_quantity_guard() {
        let mut fixture = fixture();
        fixture.position_fills[1].last_qty = Quantity::from("1.71");
        sync_order_fills_from_position(&mut fixture);
        reconcile_position_accounting(&mut fixture);
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("incomplete terminal close must fail closed");
        assert!(
            error
                .to_string()
                .contains("does not exactly close the remaining instrument-precision quantity")
        );
    }

    #[test]
    fn rejects_oversized_terminal_close_at_terminal_quantity_guard() {
        let mut fixture = fixture();
        fixture.position_fills[1].last_qty = Quantity::from("5.42");
        sync_order_fills_from_position(&mut fixture);
        reconcile_position_accounting(&mut fixture);
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("oversized terminal close must fail closed");
        assert!(
            error
                .to_string()
                .contains("does not exactly close the remaining instrument-precision quantity")
        );
    }

    #[test]
    fn accepts_partial_market_fill_closed_at_observed_fill_quantity() {
        let mut fixture = fixture();
        let mut book = OrderBook::new(fixture.instrument.id(), BookType::L2_MBP);
        book.add(
            BookOrder::new(
                OrderSide::Sell,
                Price::from("0.420"),
                Quantity::from("2.00"),
                1,
            ),
            0,
            1,
            UnixNanos::from(1),
        );
        fixture.orders[0].executable_book = Some(book);
        fixture.position_fills[0].last_qty = Quantity::from("2.00");
        fixture.position_fills[1].last_qty = Quantity::from("2.00");
        fixture.orders[1].submitted_quantity = Quantity::from("2.00");
        fixture.orders[1].effective_base_quantity = Quantity::from("2.00");
        sync_order_fills_from_position(&mut fixture);
        reconcile_position_accounting(&mut fixture);
        validate_execution_contract(&fixture.trace())
            .expect("deterministic partial fill closed at its filled quantity must pass");
    }

    #[test]
    fn rejects_wrong_commission() {
        let fixture = fixture();
        let mut trace = fixture.trace();
        trace.position_commission = Money::from("0.01 USDC");
        assert!(validate_execution_contract(&trace).is_err());
    }

    #[test]
    fn rejects_correlated_wrong_fill_and_position_commission() {
        let mut fixture = fixture();
        let commission = Money::from("0.01 USDC");
        fixture.position_fills[0].commission = Some(commission);
        sync_order_fills_from_position(&mut fixture);
        let mut position = Position::new(&fixture.instrument, fixture.position_fills[0].clone());
        position.apply(&fixture.position_fills[1]);
        fixture.realized_pnl = position
            .realized_pnl
            .expect("commission fixture should realize PnL");
        fixture.terminal_cash = fixture
            .initial_cash
            .checked_add(fixture.realized_pnl)
            .expect("fixture terminal cash should be exact");
        fixture.position_commission = position
            .commissions
            .get(&commission.currency)
            .copied()
            .expect("commission fixture should accumulate commission");
        let error = validate_execution_contract(&fixture.trace())
            .expect_err("correlated non-zero commission must violate the zero-fee assumption");
        assert!(error.to_string().contains("explicit fixture assumption"));
    }

    #[test]
    fn rejects_canonical_config_bytes_hash_integrity_mismatch() {
        let mut fixture = fixture();
        fixture.config_bytes.push(b' ');
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }
}
