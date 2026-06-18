//! Shared binary-maker settlement accounting.
//!
//! The settlement signal source is deliberately outside this module. Live and
//! backtest callers pass the resolved terminal payout per leg, and this module
//! owns the deterministic 0/1 payout accounting for maker inventory lots.

use crate::{
    bolt_v3_maker_inventory::signed_net_yes,
    bolt_v3_numeric::{UNIT_F64, ZERO_F64, is_positive_finite, sanitize_probability},
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_quoting::QuoteSide,
};

/// Terminal binary payout for the YES and NO legs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinarySettlementPayout {
    yes: f64,
    no: f64,
}

impl BinarySettlementPayout {
    /// Build a terminal payout. Exactly one leg must pay 1 and the other 0.
    pub fn new(yes: f64, no: f64) -> Option<Self> {
        if !is_terminal_payout(yes) || !is_terminal_payout(no) || yes == no {
            return None;
        }
        Some(Self { yes, no })
    }

    pub fn leg_payout(self, leg: Leg) -> f64 {
        match leg {
            Leg::Yes => self.yes,
            Leg::No => self.no,
        }
    }
}

/// One maker inventory lot to settle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinarySettlementLot {
    pub leg: Leg,
    pub side: QuoteSide,
    pub quantity: f64,
    pub entry_price: f64,
}

/// Settlement result for one lot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinarySettlementLotResult {
    pub payout_per_share: f64,
    pub terminal_value: f64,
    pub entry_cashflow: f64,
    pub realized_pnl: f64,
    pub closed_quantity: f64,
    pub closed_net_yes: f64,
}

/// Aggregate settlement result for a market.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinarySettlementResult {
    pub terminal_value: f64,
    pub entry_cashflow: f64,
    pub realized_pnl: f64,
    pub closed_quantity: f64,
    pub closed_net_yes: f64,
}

impl BinarySettlementResult {
    pub fn flat() -> Self {
        Self {
            terminal_value: ZERO_F64,
            entry_cashflow: ZERO_F64,
            realized_pnl: ZERO_F64,
            closed_quantity: ZERO_F64,
            closed_net_yes: ZERO_F64,
        }
    }

    fn add_lot(&mut self, lot: BinarySettlementLotResult) -> Option<()> {
        self.terminal_value = finite_sum(self.terminal_value, lot.terminal_value)?;
        self.entry_cashflow = finite_sum(self.entry_cashflow, lot.entry_cashflow)?;
        self.realized_pnl = finite_sum(self.realized_pnl, lot.realized_pnl)?;
        self.closed_quantity = finite_sum(self.closed_quantity, lot.closed_quantity)?;
        self.closed_net_yes = finite_sum(self.closed_net_yes, lot.closed_net_yes)?;
        Some(())
    }
}

/// Settle one maker inventory lot against the terminal payout.
pub fn settle_binary_lot(
    payout: BinarySettlementPayout,
    lot: BinarySettlementLot,
) -> Option<BinarySettlementLotResult> {
    let quantity = sanitize_quantity(lot.quantity)?;
    let entry_price = sanitize_probability(lot.entry_price)?;
    let payout_per_share = payout.leg_payout(lot.leg);
    let terminal_value = checked_product(quantity, payout_per_share)?;
    let entry_value = checked_product(quantity, entry_price)?;
    let entry_cashflow = match lot.side {
        QuoteSide::Buy => -entry_value,
        QuoteSide::Sell => entry_value,
    };
    let realized_pnl = match lot.side {
        QuoteSide::Buy => finite_difference(terminal_value, entry_value)?,
        QuoteSide::Sell => finite_difference(entry_value, terminal_value)?,
    };
    Some(BinarySettlementLotResult {
        payout_per_share,
        terminal_value,
        entry_cashflow,
        realized_pnl,
        closed_quantity: quantity,
        closed_net_yes: signed_net_yes(lot.leg, lot.side, quantity)?,
    })
}

/// Settle every maker lot for one resolved binary market.
pub fn settle_binary_lots(
    payout: BinarySettlementPayout,
    lots: &[BinarySettlementLot],
) -> Option<BinarySettlementResult> {
    let mut result = BinarySettlementResult::flat();
    for lot in lots {
        result.add_lot(settle_binary_lot(payout, *lot)?)?;
    }
    Some(result)
}

fn is_terminal_payout(value: f64) -> bool {
    value == ZERO_F64 || value == UNIT_F64
}

fn sanitize_quantity(value: f64) -> Option<f64> {
    if is_positive_finite(value) {
        Some(value)
    } else {
        None
    }
}

fn checked_product(lhs: f64, rhs: f64) -> Option<f64> {
    let value = lhs * rhs;
    value.is_finite().then_some(value)
}

fn finite_sum(lhs: f64, rhs: f64) -> Option<f64> {
    let value = lhs + rhs;
    value.is_finite().then_some(value)
}

fn finite_difference(lhs: f64, rhs: f64) -> Option<f64> {
    let value = lhs - rhs;
    value.is_finite().then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f64 = 1e-9;

    #[test]
    fn terminal_payout_requires_exactly_one_winning_leg() {
        assert_eq!(
            BinarySettlementPayout::new(1.0, 0.0).map(|payout| payout.leg_payout(Leg::Yes)),
            Some(1.0)
        );
        assert_eq!(
            BinarySettlementPayout::new(0.0, 1.0).map(|payout| payout.leg_payout(Leg::No)),
            Some(1.0)
        );
        assert_eq!(BinarySettlementPayout::new(1.0, 1.0), None);
        assert_eq!(BinarySettlementPayout::new(0.0, 0.0), None);
        assert_eq!(BinarySettlementPayout::new(0.5, 0.5), None);
        assert_eq!(BinarySettlementPayout::new(f64::NAN, 1.0), None);
    }

    #[test]
    fn settled_yes_buy_books_terminal_payout_minus_entry_cost() {
        let payout = BinarySettlementPayout::new(1.0, 0.0).expect("up outcome");
        let result = settle_binary_lot(
            payout,
            BinarySettlementLot {
                leg: Leg::Yes,
                side: QuoteSide::Buy,
                quantity: 4.0,
                entry_price: 0.42,
            },
        )
        .expect("valid lot");

        assert_eq!(result.payout_per_share, 1.0);
        assert!((result.terminal_value - 4.0).abs() < EPSILON);
        assert!((result.entry_cashflow - (-1.68)).abs() < EPSILON);
        assert!((result.realized_pnl - 2.32).abs() < EPSILON);
        assert!((result.closed_net_yes - 4.0).abs() < EPSILON);
    }

    #[test]
    fn settled_losing_no_buy_books_loss_to_zero_payout() {
        let payout = BinarySettlementPayout::new(1.0, 0.0).expect("up outcome");
        let result = settle_binary_lot(
            payout,
            BinarySettlementLot {
                leg: Leg::No,
                side: QuoteSide::Buy,
                quantity: 3.0,
                entry_price: 0.37,
            },
        )
        .expect("valid lot");

        assert_eq!(result.payout_per_share, 0.0);
        assert_eq!(result.terminal_value, 0.0);
        assert!((result.entry_cashflow - (-1.11)).abs() < EPSILON);
        assert!((result.realized_pnl - (-1.11)).abs() < EPSILON);
        assert!((result.closed_net_yes - (-3.0)).abs() < EPSILON);
    }

    #[test]
    fn settled_short_lot_reverses_entry_and_terminal_cashflows() {
        let payout = BinarySettlementPayout::new(0.0, 1.0).expect("down outcome");
        let result = settle_binary_lot(
            payout,
            BinarySettlementLot {
                leg: Leg::Yes,
                side: QuoteSide::Sell,
                quantity: 2.0,
                entry_price: 0.64,
            },
        )
        .expect("valid lot");

        assert_eq!(result.payout_per_share, 0.0);
        assert_eq!(result.terminal_value, 0.0);
        assert!((result.entry_cashflow - 1.28).abs() < EPSILON);
        assert!((result.realized_pnl - 1.28).abs() < EPSILON);
        assert!((result.closed_net_yes - (-2.0)).abs() < EPSILON);
    }

    #[test]
    fn settlement_aggregates_lots_and_closes_net_exposure() {
        let payout = BinarySettlementPayout::new(1.0, 0.0).expect("up outcome");
        let result = settle_binary_lots(
            payout,
            &[
                BinarySettlementLot {
                    leg: Leg::Yes,
                    side: QuoteSide::Buy,
                    quantity: 4.0,
                    entry_price: 0.42,
                },
                BinarySettlementLot {
                    leg: Leg::No,
                    side: QuoteSide::Buy,
                    quantity: 3.0,
                    entry_price: 0.37,
                },
            ],
        )
        .expect("valid lots");

        assert!((result.terminal_value - 4.0).abs() < EPSILON);
        assert!((result.entry_cashflow - (-2.79)).abs() < EPSILON);
        assert!((result.realized_pnl - 1.21).abs() < EPSILON);
        assert!((result.closed_quantity - 7.0).abs() < EPSILON);
        assert!((result.closed_net_yes - 1.0).abs() < EPSILON);
    }

    #[test]
    fn settlement_fails_closed_on_invalid_lot_inputs_without_partial_result() {
        let payout = BinarySettlementPayout::new(1.0, 0.0).expect("up outcome");
        for bad_quantity in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                settle_binary_lot(
                    payout,
                    BinarySettlementLot {
                        leg: Leg::Yes,
                        side: QuoteSide::Buy,
                        quantity: bad_quantity,
                        entry_price: 0.5,
                    },
                ),
                None
            );
        }
        for bad_price in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
            assert_eq!(
                settle_binary_lot(
                    payout,
                    BinarySettlementLot {
                        leg: Leg::Yes,
                        side: QuoteSide::Buy,
                        quantity: 1.0,
                        entry_price: bad_price,
                    },
                ),
                None
            );
        }
        assert_eq!(
            settle_binary_lots(
                payout,
                &[
                    BinarySettlementLot {
                        leg: Leg::Yes,
                        side: QuoteSide::Buy,
                        quantity: 1.0,
                        entry_price: 0.5,
                    },
                    BinarySettlementLot {
                        leg: Leg::No,
                        side: QuoteSide::Buy,
                        quantity: f64::INFINITY,
                        entry_price: 0.5,
                    },
                ],
            ),
            None
        );
    }
}
