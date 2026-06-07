//! Shared maker inventory accumulator.

use crate::{
    bolt_v3_numeric::{ZERO_F64, is_positive_finite},
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_quoting::QuoteSide,
};

/// Net directional maker inventory accumulated from confirmed fills.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MakerInventory {
    net_position: f64,
}

impl MakerInventory {
    /// A flat book with zero directional exposure.
    pub fn flat() -> Self {
        Self {
            net_position: ZERO_F64,
        }
    }

    /// Current net directional position. Positive means net long YES/up.
    pub fn net_position(&self) -> f64 {
        self.net_position
    }

    /// Fold one confirmed fill into net directional inventory.
    pub fn apply_fill(&mut self, leg: Leg, side: QuoteSide, qty: f64) -> bool {
        if !is_positive_finite(qty) {
            return false;
        }

        let signed = match (leg, side) {
            (Leg::Yes, QuoteSide::Buy) => qty,
            (Leg::Yes, QuoteSide::Sell) => -qty,
            (Leg::No, QuoteSide::Buy) => -qty,
            (Leg::No, QuoteSide::Sell) => qty,
        };
        let next_position = self.net_position + signed;
        if !next_position.is_finite() {
            return false;
        }
        self.net_position = next_position;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_quote_lifecycle::Leg;
    use crate::bolt_v3_quoting::QuoteSide;

    const EPSILON: f64 = 1e-9;

    #[test]
    fn a_fresh_book_is_flat() {
        assert_eq!(MakerInventory::flat().net_position(), 0.0);
    }

    #[test]
    fn yes_buy_lengthens_and_no_buy_shortens_the_position() {
        let mut book = MakerInventory::flat();

        assert!(book.apply_fill(Leg::Yes, QuoteSide::Buy, 3.0));
        assert!((book.net_position() - 3.0).abs() < EPSILON);
        assert!(book.apply_fill(Leg::No, QuoteSide::Buy, 2.0));
        assert!((book.net_position() - 1.0).abs() < EPSILON);
    }

    #[test]
    fn sells_reverse_each_leg_sign() {
        let mut book = MakerInventory::flat();

        assert!(book.apply_fill(Leg::Yes, QuoteSide::Buy, 5.0));
        assert!(book.apply_fill(Leg::Yes, QuoteSide::Sell, 2.0));
        assert!((book.net_position() - 3.0).abs() < EPSILON);
        assert!(book.apply_fill(Leg::No, QuoteSide::Sell, 1.0));
        assert!((book.net_position() - 4.0).abs() < EPSILON);
    }

    #[test]
    fn matched_binary_pair_nets_to_flat() {
        let mut book = MakerInventory::flat();

        assert!(book.apply_fill(Leg::Yes, QuoteSide::Buy, 4.0));
        assert!(book.apply_fill(Leg::No, QuoteSide::Buy, 4.0));
        assert!(book.net_position().abs() < EPSILON);
    }

    #[test]
    fn invalid_quantities_are_rejected_without_mutating_inventory() {
        let mut book = MakerInventory::flat();

        assert!(book.apply_fill(Leg::Yes, QuoteSide::Buy, 2.0));
        assert!(!book.apply_fill(Leg::Yes, QuoteSide::Buy, 0.0));
        assert!(!book.apply_fill(Leg::Yes, QuoteSide::Buy, -1.0));
        assert!(!book.apply_fill(Leg::No, QuoteSide::Buy, f64::NAN));
        assert!(!book.apply_fill(Leg::No, QuoteSide::Buy, f64::INFINITY));
        assert!((book.net_position() - 2.0).abs() < EPSILON);
    }

    #[test]
    fn inventory_overflow_is_rejected_without_mutating_inventory() {
        let mut book = MakerInventory::flat();

        assert!(book.apply_fill(Leg::Yes, QuoteSide::Buy, f64::MAX));
        assert_eq!(book.net_position(), f64::MAX);
        assert!(!book.apply_fill(Leg::Yes, QuoteSide::Buy, f64::MAX));
        assert_eq!(book.net_position(), f64::MAX);
    }
}
