//! Pure per-token binary settlement accounting for the binary-oracle maker
//! (W4 — terminal 0/1 payout + realized P&L, FR-030 / FR-004 / US4).
//!
//! A resting two-sided maker is *structurally* left holding inventory into a
//! binary's expiry, and — because it quotes and fills on **both** legs — that
//! inventory is generally a position in the YES token *and* a position in the NO
//! token at the same time. NT does **not** auto-settle a binary: resolution
//! surfaces only as an `InstrumentStatus::Close` data event, and the on-chain
//! redemption that turns a winning token into cash is a shell-side action. This
//! module is the pure half of that path: given the two held token lots and the
//! resolved outcome, it books the terminal 0/1 payout and the realized P&L. The
//! shell handles the NT event, the redemption call, and book closure (out of
//! scope here); the **same** primitive is reused by the pre-live backtest so
//! live and backtest never settle on two different paths (FR-004 — no dual
//! settlement path).
//!
//! ## Per-token, not net — why
//!
//! On Polymarket the YES and NO tokens are **separate instruments**, each with
//! its own NT position and cost basis. At resolution they settle independently:
//! the winning token pays [`UNIT_F64`] per share, the losing token pays
//! [`ZERO_F64`]. Collapsing the book to a single net YES-equivalent quantity
//! before settling is *lossy* and wrong for a two-sided maker: a directionally
//! matched pair (e.g. long 5 YES *and* long 5 NO) nets to zero, yet at expiry it
//! still redeems 5 winning tokens for cash — a net-only model would book that
//! 5.0 receipt as 0 and report the entire cash outlay as a phantom loss. So
//! settlement reads the two lots separately and sums their independent payouts.
//!
//! Sign convention: each lot's `position` is the signed token quantity from NT's
//! per-instrument net position (positive = long the token, negative = short it),
//! and `cost_basis` is the net cash already paid to establish it (positive = cash
//! paid out, negative = net cash received). NT owns position + PnL (NT-FIRST), so
//! the shell sources both fields from NT's two instrument positions rather than
//! re-deriving them here. Realized P&L is the terminal payout minus the combined
//! cost basis: settlement is booked at the 0/1 payout, never mark-to-mid (US4 /
//! SC-005).
//!
//! Pure: no NautilusTrader type, no async, no I/O. Fail-closed: a non-finite
//! input — or a non-finite product/sum — yields `None` rather than a poisoned
//! booking; a NaN payout must never silently enter P&L accounting. The 0/1
//! payout values come from [`crate::bolt_v3_numeric`]; no inline `0.0`/`1.0`
//! runtime literal on the production path.

use crate::bolt_v3_numeric::{UNIT_F64, ZERO_F64};
use crate::strategies::quote_lifecycle::Leg;

/// The resolved outcome of a binary market: which outcome token settles to
/// [`UNIT_F64`] (the other settles to [`ZERO_F64`]).
///
/// Reuses the [`Leg`] vocabulary so the resolution shares one outcome alphabet
/// with the quote legs, the inventory book, and the reconnect reconciliation (no
/// parallel YES/NO enum): [`Leg::Yes`] = the YES (leg-a / "up") token resolved
/// true; [`Leg::No`] = the NO token resolved true.
pub type SettlementOutcome = Leg;

/// One outcome token's held lot at resolution: the signed token position and the
/// net cash already paid to establish it.
///
/// Both fields are sourced from NT's per-instrument accounting for that token
/// (NT-FIRST): `position` is the signed net token quantity, `cost_basis` the net
/// signed cash committed to it. Holding the two lots separately (rather than a
/// single net) is exactly what lets [`settle`] book a both-sides inventory
/// correctly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenLot {
    /// Signed token quantity held into expiry: positive = long the token,
    /// negative = short it.
    pub position: f64,
    /// Net cash paid to establish `position`: positive = cash paid out to acquire
    /// the lot, negative = net cash received (e.g. from a short).
    pub cost_basis: f64,
}

impl TokenLot {
    /// Build a lot from NT's signed position quantity and net cash cost basis.
    pub fn new(position: f64, cost_basis: f64) -> Self {
        Self {
            position,
            cost_basis,
        }
    }

    /// A token the maker holds no position in and has committed no cash to.
    ///
    /// Provided instead of a `Default` impl: the bolt-v3 legacy-default fence
    /// forbids `Default` on the production surface, and "no lot" is an explicit,
    /// named state — never a silently inherited zeroed value.
    pub fn flat() -> Self {
        Self {
            position: ZERO_F64,
            cost_basis: ZERO_F64,
        }
    }

    /// Both fields are finite — the precondition [`settle`] requires before it
    /// trusts a lot in the payout arithmetic.
    fn is_finite(&self) -> bool {
        self.position.is_finite() && self.cost_basis.is_finite()
    }
}

/// The terminal booking produced by settling the held token lots at resolution.
///
/// Constructed only via [`settle`] (no `Default`: the legacy-default fence
/// forbids a `Default` impl on the production surface, and a settlement must be
/// derived from real lots + an outcome, never inherited as a zeroed booking).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SettlementBooking {
    /// Terminal cash value of the held inventory at the 0/1 payout: the winning
    /// token's signed position times [`UNIT_F64`], the losing token contributing
    /// [`ZERO_F64`]. Positive when the maker is net long the winning token,
    /// negative when net short it (the maker owes the winning-token value).
    payout: f64,
    /// Realized P&L of the resolution: terminal `payout` minus the combined cash
    /// paid to acquire both lots. Booked at the 0/1 payout, not mark-to-mid.
    realized_pnl: f64,
}

impl SettlementBooking {
    /// Terminal payout booked for the held inventory at the 0/1 resolution.
    pub fn payout(&self) -> f64 {
        self.payout
    }

    /// Realized settlement P&L (`payout − combined cost basis`).
    pub fn realized_pnl(&self) -> f64 {
        self.realized_pnl
    }
}

/// Settle the maker's held YES and NO token lots at a binary resolution.
///
/// The winning outcome token pays [`UNIT_F64`] per share and the losing token
/// pays [`ZERO_F64`], applied to each lot's signed position independently:
///
/// ```text
/// payout       = yes.position × (UNIT_F64 if YES won else ZERO_F64)
///              + no.position  × (UNIT_F64 if NO  won else ZERO_F64)
/// realized_pnl = payout − (yes.cost_basis + no.cost_basis)
/// ```
///
/// Because the two lots settle independently, a both-sides inventory books
/// correctly: a long-5-YES + long-5-NO pair redeems its 5 winning tokens for
/// `5 × UNIT_F64` regardless of which side wins, and the P&L is that receipt less
/// the combined cash paid — never a phantom loss from a netted-to-zero position.
///
/// Returns `None` (fail-closed) when any lot field is not finite, or when the
/// resulting `payout`/`realized_pnl` is not finite — a non-finite booking must
/// never silently enter P&L accounting.
pub fn settle(
    yes: TokenLot,
    no: TokenLot,
    outcome: SettlementOutcome,
) -> Option<SettlementBooking> {
    if !yes.is_finite() || !no.is_finite() {
        return None;
    }
    // Each token's terminal per-share value: a unit for the winning token, zero
    // for the losing one. Exactly one outcome wins, so exactly one lot is paid.
    let (yes_token_value, no_token_value) = match outcome {
        Leg::Yes => (UNIT_F64, ZERO_F64),
        Leg::No => (ZERO_F64, UNIT_F64),
    };
    let payout = yes.position * yes_token_value + no.position * no_token_value;
    let cost_basis = yes.cost_basis + no.cost_basis;
    let realized_pnl = payout - cost_basis;
    if !payout.is_finite() || !realized_pnl.is_finite() {
        return None;
    }
    Some(SettlementBooking {
        payout,
        realized_pnl,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tolerance for accumulated-float comparisons.
    const EPSILON: f64 = 1e-9;

    #[test]
    fn a_matched_pair_settles_to_its_winning_token_value_on_either_outcome() {
        // The regression the net-equivalent model got wrong: long 5 YES (paid
        // 2.75) AND long 5 NO (paid 2.25) = 5.00 cash, holding a matched pair.
        // Whichever side wins, the 5 winning tokens redeem for 5.00, so realized
        // P&L is ~0 — NOT a 5.00 phantom loss from a netted-to-zero position.
        let yes = TokenLot::new(5.0, 2.75);
        let no = TokenLot::new(5.0, 2.25);
        for outcome in [Leg::Yes, Leg::No] {
            let booking = settle(yes, no, outcome).expect("matched pair settles");
            assert!(
                (booking.payout() - 5.0).abs() < EPSILON,
                "the 5 winning tokens redeem for 5.0 on {outcome:?}"
            );
            assert!(
                booking.realized_pnl().abs() < EPSILON,
                "a matched pair bought at fair books ~0 P&L on {outcome:?}"
            );
        }
    }

    #[test]
    fn long_yes_pays_a_unit_per_share_when_yes_wins_and_books_the_gain() {
        // Bought 10 YES for 4.0; YES resolves true -> each share pays UNIT_F64.
        let booking =
            settle(TokenLot::new(10.0, 4.0), TokenLot::flat(), Leg::Yes).expect("finite settles");
        assert!((booking.payout() - 10.0).abs() < EPSILON);
        assert!((booking.realized_pnl() - 6.0).abs() < EPSILON); // 10.0 - 4.0
    }

    #[test]
    fn long_yes_pays_zero_when_no_wins_and_books_the_full_cost_as_loss() {
        let booking =
            settle(TokenLot::new(10.0, 4.0), TokenLot::flat(), Leg::No).expect("finite settles");
        assert!(booking.payout().abs() < EPSILON);
        assert!((booking.realized_pnl() - (-4.0)).abs() < EPSILON); // 0 - 4.0
    }

    #[test]
    fn long_no_pays_a_unit_per_share_when_no_wins_and_books_the_gain() {
        // The case the YES-equivalent model booked as a LOSS: bought 7 NO for 3.0;
        // NO resolves true -> the 7 NO tokens redeem for 7.0, a +4.0 gain.
        let booking =
            settle(TokenLot::flat(), TokenLot::new(7.0, 3.0), Leg::No).expect("finite settles");
        assert!((booking.payout() - 7.0).abs() < EPSILON);
        assert!((booking.realized_pnl() - 4.0).abs() < EPSILON); // 7.0 - 3.0
    }

    #[test]
    fn long_no_pays_zero_when_yes_wins_and_books_the_full_cost_as_loss() {
        let booking =
            settle(TokenLot::flat(), TokenLot::new(7.0, 3.0), Leg::Yes).expect("finite settles");
        assert!(booking.payout().abs() < EPSILON);
        assert!((booking.realized_pnl() - (-3.0)).abs() < EPSILON); // 0 - 3.0
    }

    #[test]
    fn a_short_winning_token_owes_its_unit_value() {
        // Sold 10 YES short, receiving 4.0 cash -> cost_basis is the negative
        // outlay. YES resolves true: the short owes UNIT_F64 per share.
        let booking =
            settle(TokenLot::new(-10.0, -4.0), TokenLot::flat(), Leg::Yes).expect("finite settles");
        assert!((booking.payout() - (-10.0)).abs() < EPSILON);
        // Realized = -10.0 - (-4.0) = -6.0 (sold at 0.40, settles at 1.0).
        assert!((booking.realized_pnl() - (-6.0)).abs() < EPSILON);
        // And when the shorted token loses, the kept cash is the whole gain.
        let won =
            settle(TokenLot::new(-10.0, -4.0), TokenLot::flat(), Leg::No).expect("finite settles");
        assert!(won.payout().abs() < EPSILON);
        assert!((won.realized_pnl() - 4.0).abs() < EPSILON); // 0 - (-4.0)
    }

    #[test]
    fn a_flat_book_pays_zero_and_books_zero_pnl_on_either_outcome() {
        for outcome in [Leg::Yes, Leg::No] {
            let booking =
                settle(TokenLot::flat(), TokenLot::flat(), outcome).expect("flat settles");
            assert!(booking.payout().abs() < EPSILON);
            assert!(booking.realized_pnl().abs() < EPSILON);
        }
    }

    #[test]
    fn an_unbalanced_both_sides_book_settles_each_lot_independently() {
        // Long 8 YES (paid 4.0) AND long 3 NO (paid 1.2): the two lots settle on
        // their own outcomes, not on a net of 5.
        let yes = TokenLot::new(8.0, 4.0);
        let no = TokenLot::new(3.0, 1.2);
        let yes_wins = settle(yes, no, Leg::Yes).expect("settles");
        assert!((yes_wins.payout() - 8.0).abs() < EPSILON); // 8*1 + 3*0
        assert!((yes_wins.realized_pnl() - (8.0 - 5.2)).abs() < EPSILON);
        let no_wins = settle(yes, no, Leg::No).expect("settles");
        assert!((no_wins.payout() - 3.0).abs() < EPSILON); // 8*0 + 3*1
        assert!((no_wins.realized_pnl() - (3.0 - 5.2)).abs() < EPSILON);
    }

    #[test]
    fn the_payout_minus_combined_cost_identity_holds() {
        // Property: realized_pnl + (yes.cost + no.cost) == payout for any finite
        // lots the module accepts (the booking is internally consistent).
        let positions = [-7.0_f64, ZERO_F64, 0.5, 12.0];
        let costs = [-3.0_f64, ZERO_F64, 0.25, 9.0];
        for &yp in &positions {
            for &yc in &costs {
                for &np in &positions {
                    for &nc in &costs {
                        for outcome in [Leg::Yes, Leg::No] {
                            let booking =
                                settle(TokenLot::new(yp, yc), TokenLot::new(np, nc), outcome)
                                    .expect("finite lots settle");
                            assert!(
                                (booking.realized_pnl() + (yc + nc) - booking.payout()).abs()
                                    < EPSILON,
                                "identity broken for yes=({yp},{yc}) no=({np},{nc}) {outcome:?}",
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_winning_token_pays_a_unit_and_the_losing_token_pays_zero_per_share() {
        // One share on each side: exactly UNIT_F64 from the winner, ZERO_F64 from
        // the loser, per share.
        let one_each = (
            TokenLot::new(UNIT_F64, ZERO_F64),
            TokenLot::new(UNIT_F64, ZERO_F64),
        );
        let yes_won = settle(one_each.0, one_each.1, Leg::Yes).expect("settles");
        assert!((yes_won.payout() - UNIT_F64).abs() < EPSILON);
        let no_won = settle(one_each.0, one_each.1, Leg::No).expect("settles");
        assert!((no_won.payout() - UNIT_F64).abs() < EPSILON);
    }

    #[test]
    fn a_non_finite_field_fails_closed_to_none() {
        let live = TokenLot::new(10.0, 4.0);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(settle(TokenLot::new(bad, 4.0), live, Leg::Yes).is_none());
            assert!(settle(TokenLot::new(10.0, bad), live, Leg::Yes).is_none());
            assert!(settle(live, TokenLot::new(bad, 1.0), Leg::No).is_none());
            assert!(settle(live, TokenLot::new(1.0, bad), Leg::No).is_none());
        }
    }
}
