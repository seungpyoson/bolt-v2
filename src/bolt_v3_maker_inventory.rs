//! Binary-market maker inventory and composite exposure.
//!
//! Two distinct net-YES exposure views share ONE No-leg sign adapter
//! ([`signed_net_yes`]) so they can never disagree on sign:
//! - [`MakerInventory`] — the confirmed-fill-only accumulator (a single
//!   `net_position` folded from `apply_fill`). Per §16#4 it is at most ONE input
//!   to the composite, never the accumulator over the union (avoids Rule #6 dual
//!   inventory state).
//! - [`CompositeExposure`] — the single net-new snapshot the admission gate reads:
//!   the confirmed-fill net-YES PLUS every still-open and still-inflight maker
//!   order, covering the pending exposure NT Portfolio's `net_position` omits.

use crate::{
    bolt_v3_numeric::{ZERO_F64, is_positive_finite},
    bolt_v3_quote_lifecycle::Leg,
    bolt_v3_quoting::QuoteSide,
};

/// The signed net-YES contribution of one binary maker flow of `qty` shares on
/// `leg`/`side`, in net-YES units. This is THE No-leg sign adapter — the single
/// place the `(leg, side) -> sign` mapping lives, shared by the confirmed-fill
/// [`MakerInventory`] and the [`CompositeExposure`] snapshot so the two can never
/// disagree on sign. A YES buy or a NO sell lengthens net-YES (`+`); a YES sell or
/// a NO buy shortens it (`-`). Fails closed (`None`) on a non-positive or
/// non-finite quantity, so an invalid flow can never perturb exposure.
pub(crate) fn signed_net_yes(leg: Leg, side: QuoteSide, qty: f64) -> Option<f64> {
    if !is_positive_finite(qty) {
        return None;
    }
    Some(match (leg, side) {
        (Leg::Yes, QuoteSide::Buy) | (Leg::No, QuoteSide::Sell) => qty,
        (Leg::Yes, QuoteSide::Sell) | (Leg::No, QuoteSide::Buy) => -qty,
    })
}

/// Net directional binary maker inventory accumulated from confirmed fills.
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

    /// Fold one confirmed fill into net directional inventory through the shared
    /// [`signed_net_yes`] adapter; rejects (returns `false`, no mutation) a
    /// non-positive/non-finite quantity or an overflow to a non-finite position.
    pub fn apply_fill(&mut self, leg: Leg, side: QuoteSide, qty: f64) -> bool {
        let Some(signed) = signed_net_yes(leg, side, qty) else {
            return false;
        };
        let next_position = self.net_position + signed;
        if !next_position.is_finite() {
            return false;
        }
        self.net_position = next_position;
        true
    }
}

/// One still-open or still-inflight maker order's pending exposure: `qty` resting
/// shares on `leg`/`side`, reconciled to net-YES through [`signed_net_yes`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureLeg {
    pub leg: Leg,
    pub side: QuoteSide,
    pub qty: f64,
}

/// The single net-new composite maker exposure snapshot the admission gate reads
/// (§16#4): the confirmed-fill net-YES (NT Portfolio filled positions — and at
/// most [`MakerInventory`] as one cross-check input, never the accumulator) PLUS
/// the net-YES of every still-open and still-inflight maker order. NT Portfolio's
/// `net_position` omits inflight orders, so a fills-only reading under-counts
/// pending exposure on a thin CLOB; folding open and inflight orders through the
/// SAME No-leg adapter as fills closes that gap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompositeExposure {
    net_yes: f64,
}

impl CompositeExposure {
    /// Build the snapshot from the already-reconciled confirmed-fill net-YES plus
    /// the still-open and still-inflight maker orders. `filled_net_yes` arrives
    /// pre-reconciled because NT Portfolio positions carry a `PositionSide`, not the
    /// `QuoteSide` of a resting order; the open/inflight orders are reconciled here
    /// through [`signed_net_yes`]. Fails closed (`None`) when `filled_net_yes` is
    /// non-finite, any order quantity is non-positive/non-finite, or the running sum
    /// overflows to non-finite — so a corrupt input can never present a
    /// smaller-than-true exposure to the gate.
    pub fn snapshot(
        filled_net_yes: f64,
        open_orders: &[ExposureLeg],
        inflight_orders: &[ExposureLeg],
    ) -> Option<Self> {
        if !filled_net_yes.is_finite() {
            return None;
        }
        let mut net_yes = filled_net_yes;
        for order in open_orders.iter().chain(inflight_orders.iter()) {
            net_yes += signed_net_yes(order.leg, order.side, order.qty)?;
            if !net_yes.is_finite() {
                return None;
            }
        }
        Some(Self { net_yes })
    }

    /// Net directional exposure in net-YES units. Positive means net long YES/up.
    pub fn net_yes(&self) -> f64 {
        self.net_yes
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

    #[test]
    fn signed_net_yes_maps_each_leg_side_to_the_right_sign() {
        assert_eq!(signed_net_yes(Leg::Yes, QuoteSide::Buy, 2.0), Some(2.0));
        assert_eq!(signed_net_yes(Leg::Yes, QuoteSide::Sell, 2.0), Some(-2.0));
        assert_eq!(signed_net_yes(Leg::No, QuoteSide::Buy, 2.0), Some(-2.0));
        assert_eq!(signed_net_yes(Leg::No, QuoteSide::Sell, 2.0), Some(2.0));
    }

    #[test]
    fn signed_net_yes_fails_closed_on_non_positive_or_non_finite_qty() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(signed_net_yes(Leg::Yes, QuoteSide::Buy, bad), None);
        }
    }

    #[test]
    fn composite_counts_open_order_pending_exposure_that_fills_alone_miss() {
        // The §16#4 raison d'être: a resting NO buy is short-YES PENDING exposure
        // that a fills-only (filled_net_yes = 0) reading reports as flat. The
        // composite must surface it; a variant that ignored open_orders returns 0.0.
        let snap = CompositeExposure::snapshot(
            0.0,
            &[ExposureLeg {
                leg: Leg::No,
                side: QuoteSide::Buy,
                qty: 2.0,
            }],
            &[],
        )
        .expect("finite inputs");
        assert!((snap.net_yes() - (-2.0)).abs() < EPSILON);
    }

    #[test]
    fn composite_counts_inflight_order_exposure_portfolio_omits() {
        // The open build item (§16#4): inflight (submitted-not-acked) orders, which
        // NT Portfolio.net_position structurally omits, must be in the snapshot. A
        // variant that ignored inflight_orders returns 0.0 here.
        let snap = CompositeExposure::snapshot(
            0.0,
            &[],
            &[ExposureLeg {
                leg: Leg::Yes,
                side: QuoteSide::Buy,
                qty: 3.0,
            }],
        )
        .expect("finite inputs");
        assert!((snap.net_yes() - 3.0).abs() < EPSILON);
    }

    #[test]
    fn composite_reconciles_the_no_leg_sign_for_orders() {
        // A NO sell lengthens net-YES (+), a NO buy shortens it (-) — same adapter
        // as a fill. Pins the sign so a flipped No-leg adapter is caught.
        let long = CompositeExposure::snapshot(
            0.0,
            &[ExposureLeg {
                leg: Leg::No,
                side: QuoteSide::Sell,
                qty: 2.0,
            }],
            &[],
        )
        .expect("finite")
        .net_yes();
        let short = CompositeExposure::snapshot(
            0.0,
            &[ExposureLeg {
                leg: Leg::No,
                side: QuoteSide::Buy,
                qty: 2.0,
            }],
            &[],
        )
        .expect("finite")
        .net_yes();
        assert!((long - 2.0).abs() < EPSILON);
        assert!((short - (-2.0)).abs() < EPSILON);
    }

    #[test]
    fn composite_sums_filled_open_and_inflight() {
        // filled +5, open NO buy 2 (-2), inflight YES sell 1 (-1) => +2.
        let snap = CompositeExposure::snapshot(
            5.0,
            &[ExposureLeg {
                leg: Leg::No,
                side: QuoteSide::Buy,
                qty: 2.0,
            }],
            &[ExposureLeg {
                leg: Leg::Yes,
                side: QuoteSide::Sell,
                qty: 1.0,
            }],
        )
        .expect("finite inputs");
        assert!((snap.net_yes() - 2.0).abs() < EPSILON);
    }

    #[test]
    fn composite_fails_closed_on_non_finite_filled_or_order_quantities() {
        assert!(CompositeExposure::snapshot(f64::NAN, &[], &[]).is_none());
        assert!(CompositeExposure::snapshot(f64::INFINITY, &[], &[]).is_none());
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                CompositeExposure::snapshot(
                    0.0,
                    &[ExposureLeg {
                        leg: Leg::Yes,
                        side: QuoteSide::Buy,
                        qty: bad,
                    }],
                    &[],
                )
                .is_none(),
                "open order qty {bad} must fail closed"
            );
            assert!(
                CompositeExposure::snapshot(
                    0.0,
                    &[],
                    &[ExposureLeg {
                        leg: Leg::No,
                        side: QuoteSide::Sell,
                        qty: bad,
                    }],
                )
                .is_none(),
                "inflight order qty {bad} must fail closed"
            );
        }
    }

    #[test]
    fn composite_fails_closed_when_the_running_sum_overflows() {
        // filled at f64::MAX plus another large long-YES order overflows to +inf.
        assert!(
            CompositeExposure::snapshot(
                f64::MAX,
                &[ExposureLeg {
                    leg: Leg::Yes,
                    side: QuoteSide::Buy,
                    qty: f64::MAX,
                }],
                &[],
            )
            .is_none()
        );
    }

    #[test]
    fn maker_inventory_and_composite_agree_on_sign_via_the_shared_adapter() {
        // SSOT proof: feeding the SAME flow as a confirmed fill (MakerInventory) and
        // as a pending order (composite) yields the SAME signed net-YES. A fork of
        // the sign logic between the two would break this.
        let mut book = MakerInventory::flat();
        assert!(book.apply_fill(Leg::No, QuoteSide::Buy, 4.0));
        let from_fill = book.net_position();
        let from_pending = CompositeExposure::snapshot(
            0.0,
            &[ExposureLeg {
                leg: Leg::No,
                side: QuoteSide::Buy,
                qty: 4.0,
            }],
            &[],
        )
        .expect("finite")
        .net_yes();
        assert!((from_fill - from_pending).abs() < EPSILON);
        assert!((from_fill - (-4.0)).abs() < EPSILON);
    }

    #[test]
    fn maker_inventory_net_position_can_seed_the_filled_input() {
        // §16#4: MakerInventory is at most ONE input to the composite (the filled
        // net-YES), never the accumulator. Here it seeds filled_net_yes and the
        // composite adds a pending order on top.
        let mut book = MakerInventory::flat();
        assert!(book.apply_fill(Leg::Yes, QuoteSide::Buy, 6.0));
        let snap = CompositeExposure::snapshot(
            book.net_position(),
            &[ExposureLeg {
                leg: Leg::Yes,
                side: QuoteSide::Sell,
                qty: 1.0,
            }],
            &[],
        )
        .expect("finite");
        assert!((snap.net_yes() - 5.0).abs() < EPSILON);
    }
}
