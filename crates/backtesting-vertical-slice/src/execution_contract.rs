//! Exact economic validation for opted-in historical backtest diagnostics.
//!
//! This module does not model execution. It compares the runner's observed
//! economic trace with results produced by NautilusTrader's shared order-book,
//! instrument sizing, position, and account primitives.

use anyhow::{Context, Result, ensure};
use nautilus_model::{
    data::BookOrder,
    enums::OrderSide,
    identifiers::OrderId,
    instruments::InstrumentAny,
    orderbook::OrderBook,
    types::{Money, Price, Quantity},
};

use crate::hashing::sha256_hex;

#[derive(Debug, Clone)]
pub struct ExecutionFill {
    pub price: Price,
    pub quantity: Quantity,
}

pub struct ExecutionContractTrace<'a> {
    pub instrument: &'a InstrumentAny,
    pub executable_book: &'a OrderBook,
    pub order_side: OrderSide,
    pub submitted_quantity: Quantity,
    pub quote_quantity: bool,
    pub effective_base_quantity: Quantity,
    pub fills: &'a [ExecutionFill],
    pub settlement_price: Price,
    pub exit_price: Price,
    pub initial_cash: Money,
    pub terminal_cash: Money,
    pub realized_pnl: Money,
    pub fill_commissions: &'a [Money],
    pub position_commission: Money,
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
        .price;
    let expected_fills = trace.executable_book.simulate_fills(&BookOrder::new(
        trace.order_side,
        limit_price,
        trace.effective_base_quantity,
        OrderId::from(0),
    ));
    ensure!(
        expected_fills.len() == trace.fills.len()
            && expected_fills.iter().zip(trace.fills).all(
                |((expected_price, expected_quantity), observed)| {
                    *expected_price == observed.price && *expected_quantity == observed.quantity
                }
            ),
        "observed fills do not equal deterministic fills from the executable book"
    );
    ensure!(
        trace.exit_price == trace.settlement_price,
        "terminal exit price does not equal the configured settlement price"
    );

    let cash_change = trace
        .terminal_cash
        .checked_sub(trace.initial_cash)
        .context("terminal cash subtraction overflow or scale mismatch")?;
    ensure!(
        cash_change == trace.realized_pnl,
        "terminal cash change does not equal realized PnL"
    );

    let total_fill_commission = trace.fill_commissions.iter().try_fold(
        Money::zero(trace.position_commission.currency),
        |total, commission| {
            total
                .checked_add(*commission)
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
    use nautilus_model::{enums::BookType, identifiers::OrderId, instruments::stubs};

    struct Fixture {
        instrument: InstrumentAny,
        book: OrderBook,
        fills: Vec<ExecutionFill>,
        fill_commissions: Vec<Money>,
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
                settlement_price: Price::from("1.000"),
                exit_price: Price::from("1.000"),
                initial_cash: Money::from("1000000.00 USDC"),
                terminal_cash: Money::from("1000001.57 USDC"),
                realized_pnl: Money::from("1.57 USDC"),
                fill_commissions: &self.fill_commissions,
                position_commission: Money::from("0.00 USDC"),
                canonical_resolved_config_bytes: &self.config_bytes,
                canonical_resolved_config_sha256: &self.config_hash,
            }
        }
    }

    fn fixture() -> Fixture {
        let instrument = InstrumentAny::BinaryOption(stubs::binary_option());
        let mut book = OrderBook::new(instrument.id(), BookType::L2Mbp);
        book.add(
            BookOrder::new(
                OrderSide::Sell,
                Price::from("0.420"),
                Quantity::from("21.52"),
                OrderId::from(1),
            ),
            0,
            1,
            UnixNanos::from(1),
        );
        let config_bytes = br#"{"order_type":"MARKET","quote_quantity":true}"#.to_vec();
        let config_hash = sha256_hex(&config_bytes);
        Fixture {
            instrument,
            book,
            fills: vec![ExecutionFill {
                price: Price::from("0.420"),
                quantity: Quantity::from("2.71"),
            }],
            fill_commissions: vec![Money::from("0.00 USDC")],
            config_bytes,
            config_hash,
        }
    }

    #[test]
    fn accepts_exact_shared_primitive_trace() {
        validate_execution_contract(&fixture().trace()).expect("valid trace should pass");
    }

    #[test]
    fn rejects_fill_price_improvement() {
        let mut fixture = fixture();
        fixture.fills[0].price = Price::from("0.410");
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }

    #[test]
    fn rejects_depth_over_consumption() {
        let mut fixture = fixture();
        fixture.fills[0].quantity = Quantity::from("21.53");
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
        trace.terminal_cash = Money::from("1000003.14 USDC");
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
    fn rejects_wrong_commission() {
        let fixture = fixture();
        let mut trace = fixture.trace();
        trace.position_commission = Money::from("0.01 USDC");
        assert!(validate_execution_contract(&trace).is_err());
    }

    #[test]
    fn rejects_correlated_wrong_fill_and_position_commission() {
        let mut fixture = fixture();
        fixture.fill_commissions[0] = Money::from("0.01 USDC");
        let mut trace = fixture.trace();
        trace.position_commission = Money::from("0.01 USDC");
        assert!(validate_execution_contract(&trace).is_err());
    }

    #[test]
    fn rejects_config_change_with_unchanged_provenance() {
        let mut fixture = fixture();
        fixture.config_bytes.push(b' ');
        assert!(validate_execution_contract(&fixture.trace()).is_err());
    }
}
