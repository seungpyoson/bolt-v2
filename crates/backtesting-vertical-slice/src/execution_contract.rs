//! Exact economic validation for opted-in historical backtest diagnostics.
//!
//! This module does not model execution. It compares the runner's observed
//! economic trace with results produced by NautilusTrader's shared order-book,
//! instrument sizing, position, and account primitives.

use anyhow::{Context, Result, ensure};
use nautilus_model::{
    data::BookOrder,
    enums::{OrderSide, OrderType},
    events::OrderFilled,
    instruments::{Instrument, InstrumentAny},
    orderbook::OrderBook,
    position::Position,
    types::{Money, Price, Quantity},
};

use crate::hashing::sha256_hex;

pub struct ExecutionContractTrace<'a> {
    pub instrument: &'a InstrumentAny,
    pub executable_book: &'a OrderBook,
    pub order_side: OrderSide,
    pub submitted_quantity: Quantity,
    pub quote_quantity: bool,
    pub effective_base_quantity: Quantity,
    pub fills: &'a [OrderFilled],
    pub position_fills: &'a [OrderFilled],
    pub settlement_price: Price,
    pub exit_price: Price,
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
        !trace.fills.is_empty(),
        "execution contract requires at least one fill"
    );
    ensure!(
        trace.fills.iter().all(|fill| {
            fill.instrument_id == trace.instrument.id()
                && fill.order_side == trace.order_side
                && fill.order_type == OrderType::Market
        }),
        "execution contract supports only one-instrument market-order entry fills"
    );

    let reference_price = match trace.order_side {
        OrderSide::Buy => trace.executable_book.best_ask_price(),
        OrderSide::Sell => trace.executable_book.best_bid_price(),
        OrderSide::NoOrderSide => None,
    }
    .context("executable book has no opposing price")?;

    if trace.quote_quantity {
        let expected_base_quantity = trace
            .instrument
            .get_base_quantity(trace.submitted_quantity, reference_price);
        ensure!(
            trace.effective_base_quantity == expected_base_quantity,
            "quote/base conversion mismatch: submitted {} at {} resolves to {}, observed {}",
            trace.submitted_quantity,
            reference_price,
            expected_base_quantity,
            trace.effective_base_quantity,
        );
    } else {
        ensure!(
            trace.effective_base_quantity == trace.submitted_quantity,
            "base-denominated order quantity changed before execution"
        );
    }

    let limit_price = trace
        .fills
        .last()
        .context("execution contract requires at least one fill")?
        .last_px;
    let expected_fills = trace.executable_book.simulate_fills(&BookOrder::new(
        trace.order_side,
        limit_price,
        trace.effective_base_quantity,
        0,
    ));
    ensure!(
        expected_fills.len() == trace.fills.len()
            && expected_fills.iter().zip(trace.fills).all(
                |((expected_price, expected_quantity), observed)| {
                    *expected_price == observed.last_px && *expected_quantity == observed.last_qty
                }
            ),
        "observed fills do not equal deterministic fills from the executable book"
    );
    ensure!(
        trace.exit_price == trace.settlement_price,
        "terminal exit price does not equal the configured settlement price"
    );

    let (opening_fill, closing_fills) = trace
        .position_fills
        .split_first()
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
    ensure!(
        trace.position_fills.starts_with(trace.fills),
        "order entry fills do not exactly match the position entry fills"
    );
    let mut replayed_position = Position::new(trace.instrument, *opening_fill);
    for fill in closing_fills {
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
        validated_fill_count: trace.fills.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nautilus_core::UnixNanos;
    use nautilus_model::{enums::BookType, instruments::stubs};

    struct Fixture {
        instrument: InstrumentAny,
        book: OrderBook,
        fills: Vec<OrderFilled>,
        position_fills: Vec<OrderFilled>,
        initial_cash: Money,
        terminal_cash: Money,
        realized_pnl: Money,
        config_bytes: Vec<u8>,
        config_hash: String,
    }

    impl Fixture {
        fn trace(&self) -> ExecutionContractTrace<'_> {
            ExecutionContractTrace {
                instrument: &self.instrument,
                executable_book: &self.book,
                order_side: OrderSide::Buy,
                submitted_quantity: Quantity::from("1.14"),
                quote_quantity: true,
                effective_base_quantity: Quantity::from("2.71"),
                fills: &self.fills,
                position_fills: &self.position_fills,
                settlement_price: Price::from("1.000"),
                exit_price: Price::from("1.000"),
                initial_cash: self.initial_cash,
                terminal_cash: self.terminal_cash,
                realized_pnl: self.realized_pnl,
                position_commission: Money::from("0.00 USDC"),
                expected_fill_commission: Money::from("0.00 USDC"),
                canonical_resolved_config_bytes: &self.config_bytes,
                canonical_resolved_config_sha256: &self.config_hash,
            }
        }
    }

    fn fixture() -> Fixture {
        let instrument = InstrumentAny::BinaryOption(stubs::binary_option());
        let mut book = OrderBook::new(instrument.id(), BookType::L2_MBP);
        book.add(
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
        let mut position = Position::new(&instrument, entry_fill);
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
            book,
            fills: vec![entry_fill],
            position_fills: vec![entry_fill, exit_fill],
            initial_cash,
            terminal_cash,
            realized_pnl,
            config_bytes,
            config_hash,
        }
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
        )
    }

    #[test]
    fn accepts_exact_shared_primitive_trace() {
        validate_execution_contract(&fixture().trace()).expect("valid trace should pass");
    }

    #[test]
    fn rejects_fill_price_improvement() {
        let mut fixture = fixture();
        fixture.fills[0].last_px = Price::from("0.410");
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }

    #[test]
    fn rejects_depth_over_consumption() {
        let mut fixture = fixture();
        fixture.fills[0].last_qty = Quantity::from("21.53");
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }

    #[test]
    fn rejects_broken_quote_base_conversion() {
        let fixture = fixture();
        let mut trace = fixture.trace();
        trace.effective_base_quantity = Quantity::from("2.72");
        assert!(validate_execution_contract(&trace).is_err());
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
        let mut position = Position::new(&fixture.instrument, fixture.position_fills[0]);
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
    fn rejects_wrong_commission() {
        let fixture = fixture();
        let mut trace = fixture.trace();
        trace.position_commission = Money::from("0.01 USDC");
        assert!(validate_execution_contract(&trace).is_err());
    }

    #[test]
    fn rejects_correlated_wrong_fill_and_position_commission() {
        let mut fixture = fixture();
        fixture.position_fills[0].commission = Some(Money::from("0.01 USDC"));
        let mut trace = fixture.trace();
        let commission = Money::from("0.01 USDC");
        trace.position_commission = commission;
        trace.realized_pnl = trace
            .realized_pnl
            .checked_sub(commission)
            .expect("fixture commission subtraction should be exact");
        trace.terminal_cash = trace
            .initial_cash
            .checked_add(trace.realized_pnl)
            .expect("fixture terminal cash should be exact");
        assert!(validate_execution_contract(&trace).is_err());
    }

    #[test]
    fn rejects_config_change_with_unchanged_provenance() {
        let mut fixture = fixture();
        fixture.config_bytes.push(b' ');
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }
}
