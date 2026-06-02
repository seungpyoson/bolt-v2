//! Pure per-market reserved-collateral gate for the binary-oracle maker
//! (W5 — FR-040: worst-case-simultaneous-fill USDC accounting, fail-closed).
//!
//! The taker carries a single-position invariant (one open exposure at a time,
//! `binary_oracle_edge_taker::enforce_one_position_invariant`). A two-sided maker
//! breaks that assumption structurally: it rests bids on **both** outcome tokens
//! and continuously requotes, so several buy orders can be live at once. FR-040
//! replaces the one-position rule with a worst-case-fill **collateral** gate: if
//! every open buy commitment on this market filled simultaneously, would the
//! total USDC outflow exceed the per-market collateral set aside for it? If yes,
//! the new quote is refused before it is placed.
//!
//! ## What enters the worst case — and what does not
//!
//! On the Polymarket binary CLOB the maker posts USDC collateral; a resting
//! **BUY** of `size` shares at limit `price` debits exactly `price × size` USDC
//! if it fills (the same definition the order-admission layer uses for a buy's
//! cash notional — mirrored here in pure f64, not imported, because that layer is
//! `Decimal`/NT-order-typed and this one runs pre-order). The maker rests bids on
//! both legs, so the worst case is **both legs (plus any in-flight requote leg
//! plus the candidate) filling at once** — the sum of every open buy's
//! `price × size`, grossed up by the venue fee so the reserved figure equals the
//! real cash the venue will move, never an understate.
//!
//! **Already-filled inventory contributes zero additional reservation.** Its USDC
//! left the wallet at fill time; reserving against held tokens again would
//! double-count. Held inventory bounds future *sell/redemption credits* (cash
//! coming back), which are not outflows and never tighten this gate. So there is
//! deliberately **no inventory parameter** — the gate accounts only forward buy
//! commitments (resting + in-flight + the new candidate). See
//! [`crate::strategies::maker_settlement`]: held lots are future settlement
//! credits, not future debits.
//!
//! ## Strict generalization, not a parallel path
//!
//! With a single live order and a flat book, [`gate`] reduces to "is this one
//! order's grossed notional within the per-market budget?" — the same shape as
//! the per-order admission cap. It is the multi-order generalization of the
//! taker's single-position rule, sharing the `price × size` debit definition and
//! the strict-`>` cap semantics of the order-admission layer rather than forking
//! a second notion of "too much exposure" (NO DUAL PATHS). The node-global submit
//! admission still runs after it; the two are complementary, not merged.
//!
//! Pure: no NautilusTrader type, no async, no I/O. Fail-closed: any non-finite,
//! non-positive, or out-of-domain input yields `None` / [`ReservationGate::Reject`]
//! rather than a silently admitted over-commitment. All numeric invariants come
//! from [`crate::bolt_v3_numeric`]; no inline runtime literal on the production
//! path.

use crate::bolt_v3_numeric::{UNIT_F64, ZERO_F64, is_positive_finite};

/// One open BUY leg's worst-case collateral inputs: the limit `price` and the
/// `size` of a single resting bid, in-flight bid, or the candidate new bid.
///
/// Fields are private and consumed only through [`BuyCommitment::notional`], so a
/// degenerate price/size can never be summed into a reservation without passing
/// the domain check.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BuyCommitment {
    price: f64,
    size: f64,
}

impl BuyCommitment {
    /// Build a commitment from a resting/in-flight/candidate bid's limit price and
    /// share size. Validation is deferred to [`notional`](Self::notional) so the
    /// constructor stays total; an out-of-domain commitment simply contributes
    /// `None` (fail-closed) when its notional is taken.
    pub fn new(price: f64, size: f64) -> Self {
        Self { price, size }
    }

    /// This commitment's worst-case USDC debit, `price × size`, or `None`
    /// (fail-closed) when the inputs are not a valid binary buy: `price` and
    /// `size` must both be positive and finite, and `price` must lie on a binary
    /// token's `[0, 1]` probability domain — a `price > 1` is a wrong-unit feed
    /// (the same domain contract [`crate::strategies::maker_settlement`] enforces
    /// on `avg_price`), rejected rather than trusted. A non-finite product also
    /// yields `None`.
    fn notional(&self) -> Option<f64> {
        if !(is_positive_finite(self.price)
            && is_positive_finite(self.size)
            && self.price <= UNIT_F64)
        {
            return None;
        }
        let notional = self.price * self.size;
        if notional.is_finite() {
            Some(notional)
        } else {
            None
        }
    }
}

/// Sum the raw (pre-fee) worst-case debits of every open commitment.
///
/// Fail-closed: returns `None` if any commitment is out of domain
/// ([`BuyCommitment::notional`] is `None`) or the running sum goes non-finite.
fn sum_notionals(open: &[BuyCommitment]) -> Option<f64> {
    let mut sum = ZERO_F64;
    for commitment in open {
        sum += commitment.notional()?;
    }
    if sum.is_finite() { Some(sum) } else { None }
}

/// Gross a raw notional sum up by the venue fee multiplier so the reserved figure
/// is the real cash debit, never an understate.
///
/// `fee_multiplier` is `1 + fee_fraction` (e.g. `1.0` = zero fee). Fail-closed:
/// `None` unless `base` is finite and `fee_multiplier` is finite and `>= 1.0` (a
/// multiplier below 1 would *understate* the debit), or if the product is
/// non-finite.
fn gross_up(base: f64, fee_multiplier: f64) -> Option<f64> {
    if !(base.is_finite() && fee_multiplier.is_finite() && fee_multiplier >= UNIT_F64) {
        return None;
    }
    let total = base * fee_multiplier;
    if total.is_finite() { Some(total) } else { None }
}

/// The fee-grossed worst-case USDC reservation for a set of open buy commitments:
/// `(Σ price × size) × fee_multiplier`.
///
/// Fail-closed (`None`) if any commitment is out of domain, the sum is
/// non-finite, or the fee multiplier is invalid (see [`gross_up`]). Monotonic
/// non-decreasing in the number of commitments and in each `price`/`size` —
/// adding a leg never lowers the reserved figure.
pub fn worst_case_reservation(open: &[BuyCommitment], fee_multiplier: f64) -> Option<f64> {
    let base = sum_notionals(open)?;
    gross_up(base, fee_multiplier)
}

/// The verdict of the per-market reserved-collateral gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationGate {
    /// The candidate bid fits within the per-market collateral at worst case.
    Admit,
    /// The candidate bid would breach the per-market collateral at worst case, or
    /// an input was degenerate — fail-closed, do not place the order.
    Reject,
}

/// Decide whether placing `new_order` on top of the already-`open` buy
/// commitments keeps the market's worst-case-simultaneous-fill USDC outflow
/// within `available_collateral`.
///
/// Returns [`ReservationGate::Reject`] (fail-closed) when: `available_collateral`
/// is not positive and finite; any open commitment or the candidate is out of
/// domain; the grossed reservation is non-finite or the fee multiplier is invalid;
/// or the total worst-case reservation **strictly exceeds** `available_collateral`
/// (strict `>` matches the per-order admission cap, so the per-market and
/// per-order caps agree at the boundary). Otherwise [`ReservationGate::Admit`].
///
/// `open` must already exclude the leg being requoted on a cancel+resubmit venue
/// (its cancel precedes the resubmit), so a leg is never counted against itself.
/// There is no inventory parameter by design — see the module docs.
pub fn gate(
    open: &[BuyCommitment],
    new_order: BuyCommitment,
    fee_multiplier: f64,
    available_collateral: f64,
) -> ReservationGate {
    if !is_positive_finite(available_collateral) {
        return ReservationGate::Reject;
    }
    let (Some(base), Some(candidate)) = (sum_notionals(open), new_order.notional()) else {
        return ReservationGate::Reject;
    };
    let combined = base + candidate;
    let Some(total) = gross_up(combined, fee_multiplier) else {
        return ReservationGate::Reject;
    };
    if total > available_collateral {
        ReservationGate::Reject
    } else {
        ReservationGate::Admit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_FEE: f64 = 1.0;

    #[test]
    fn empty_book_reserves_exactly_the_candidate_notional() {
        // A flat book + one bid reduces to that bid's grossed notional, proving the
        // per-market gate degenerates to the per-order cap (strict generalization).
        let reservation = worst_case_reservation(&[BuyCommitment::new(0.40, 5.0)], NO_FEE);
        assert_eq!(reservation, Some(0.40 * 5.0));
    }

    #[test]
    fn sums_all_open_legs_plus_candidate_grossed_by_fee() {
        // Two resting bids + one in-flight requote leg + the candidate all debit at
        // once in the worst case; the fee multiplier grosses the whole sum.
        let open = [
            BuyCommitment::new(0.40, 5.0), // resting YES bid
            BuyCommitment::new(0.55, 4.0), // resting NO bid
            BuyCommitment::new(0.42, 5.0), // in-flight requote leg
        ];
        let fee = 1.002; // 20 bps
        let raw = 0.40 * 5.0 + 0.55 * 4.0 + 0.42 * 5.0 + 0.41 * 3.0;
        let total = worst_case_reservation(
            &[open[0], open[1], open[2], BuyCommitment::new(0.41, 3.0)],
            fee,
        );
        assert_eq!(total, Some(raw * fee));
    }

    #[test]
    fn out_of_domain_price_or_size_fails_closed() {
        // price > 1 (wrong unit), non-finite, and non-positive all reject.
        assert_eq!(BuyCommitment::new(1.01, 5.0).notional(), None);
        assert_eq!(BuyCommitment::new(f64::NAN, 5.0).notional(), None);
        assert_eq!(BuyCommitment::new(0.40, 0.0).notional(), None);
        assert_eq!(BuyCommitment::new(-0.10, 5.0).notional(), None);
        assert_eq!(
            worst_case_reservation(&[BuyCommitment::new(1.50, 5.0)], NO_FEE),
            None
        );
        assert_eq!(
            gate(&[], BuyCommitment::new(1.50, 5.0), NO_FEE, 100.0),
            ReservationGate::Reject
        );
    }

    #[test]
    fn invalid_fee_multiplier_fails_closed() {
        let one = [BuyCommitment::new(0.40, 5.0)];
        assert_eq!(worst_case_reservation(&one, 0.99), None); // < 1 would understate
        assert_eq!(worst_case_reservation(&one, f64::NAN), None);
        assert_eq!(
            gate(&[], BuyCommitment::new(0.40, 5.0), 0.99, 100.0),
            ReservationGate::Reject
        );
    }

    #[test]
    fn non_positive_collateral_fails_closed() {
        assert_eq!(
            gate(&[], BuyCommitment::new(0.40, 5.0), NO_FEE, 0.0),
            ReservationGate::Reject
        );
        assert_eq!(
            gate(&[], BuyCommitment::new(0.40, 5.0), NO_FEE, f64::NAN),
            ReservationGate::Reject
        );
    }

    #[test]
    fn boundary_admits_at_equal_rejects_one_epsilon_over() {
        // Reservation exactly == collateral admits (strict-> cap); just over rejects.
        let candidate = BuyCommitment::new(0.40, 5.0); // notional 2.0
        assert_eq!(gate(&[], candidate, NO_FEE, 2.0), ReservationGate::Admit);
        assert_eq!(
            gate(&[], candidate, NO_FEE, 2.0 - f64::EPSILON),
            ReservationGate::Reject
        );
    }

    #[test]
    fn reservation_is_monotonic_non_decreasing_in_legs() {
        // Adding a leg never lowers the reserved figure (worst case only grows).
        let base = [BuyCommitment::new(0.30, 4.0)];
        let bigger = [BuyCommitment::new(0.30, 4.0), BuyCommitment::new(0.20, 7.0)];
        let r0 = worst_case_reservation(&base, NO_FEE).unwrap();
        let r1 = worst_case_reservation(&bigger, NO_FEE).unwrap();
        assert!(r1 >= r0);
    }

    #[test]
    fn requote_excludes_self_so_no_double_count() {
        // On a cancel+resubmit venue the leg being replaced is excluded from `open`;
        // the gate then sees the true post-requote book (here: only the other leg).
        let other_leg = [BuyCommitment::new(0.55, 4.0)];
        // available exactly covers other leg + the requoted candidate, not a phantom
        // third copy of the candidate.
        let candidate = BuyCommitment::new(0.42, 5.0);
        let needed = 0.55 * 4.0 + 0.42 * 5.0;
        assert_eq!(
            gate(&other_leg, candidate, NO_FEE, needed),
            ReservationGate::Admit
        );
    }
}
