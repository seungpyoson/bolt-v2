//! Pure per-token binary settlement accounting for the binary-oracle maker
//! (W4 — terminal 0/1 payout + realized settlement P&L, FR-030 / FR-004 / US4).
//!
//! A resting two-sided maker is *structurally* left holding inventory into a
//! binary's expiry, and — because it quotes and fills on **both** legs — that
//! inventory is generally a position in the YES token *and* a position in the NO
//! token at the same time. NT does **not** auto-settle a binary: resolution
//! surfaces only as an `InstrumentStatus::Close` data event, and the on-chain
//! redemption that turns a winning token into cash is a shell-side action. This
//! module is the pure half of that path: given the two held token lots and the
//! resolved outcome, it books the terminal 0/1 payout and the **gross** realized
//! settlement P&L. The shell handles the NT event, the redemption call, and book
//! closure (out of scope here); the **same** primitive is reused by the pre-live
//! backtest so live and backtest never settle on two different paths (FR-004 — no
//! dual settlement path).
//!
//! ## Per-token, not net — why
//!
//! On Polymarket the YES and NO tokens are **separate instruments**, each with
//! its own NT position and entry price. At resolution they settle independently:
//! the winning token pays [`UNIT_F64`] per share, the losing token pays
//! [`ZERO_F64`]. Collapsing the book to a single net YES-equivalent quantity
//! before settling is *lossy* and wrong for a two-sided maker: a directionally
//! matched pair (e.g. long 5 YES *and* long 5 NO) nets to zero, yet at expiry it
//! still redeems 5 winning tokens for cash — a net-only model would book that
//! 5.0 receipt as 0 and report the entire cash outlay as a phantom loss. So
//! settlement reads the two lots separately and sums their independent payouts.
//!
//! ## The cost basis is *derived*, not trusted (enforced contract)
//!
//! Each lot is the two fields NT actually owns for that instrument (NT-FIRST):
//! the signed net `position` (positive = long the token, negative = short it) and
//! the per-share open average price `avg_price` (NT's `avg_px_open`). The notional
//! entry cost is **derived here** as `position × avg_price`, never accepted as a
//! free scalar. This is deliberate: a free "cost basis" input would let a
//! sign-flipped or per-share-vs-total feed silently re-introduce the exact
//! phantom-P&L class this module exists to kill. Because the cost is derived,
//! its sign always tracks the position's, and because `avg_price` is validated to
//! a binary token's natural domain `[0, 1]` a wrong-unit feed (e.g. total cash
//! `4.0` passed where a per-share price belongs) fails closed instead of poisoning
//! the booking.
//!
//! ## Scope of `realized_pnl` — gross of fees, single ownership
//!
//! `realized_pnl = payout − notional entry cost` of the open position. It is
//! **gross of fees by design**, because each fee component is owned exactly once
//! elsewhere (single-source-of-truth):
//! - entry/exit **commissions** are booked by NT at fill time (NT owns PnL);
//! - the **redemption-side** cost (gas/relayer) is booked by the shell's
//!   redemption path;
//! - maker **rebates** (W7) are booked by the reward layer.
//!
//! Re-subtracting any of those here would double-count against its owner. The
//! maker's *total* realized P&L is this gross settlement figure composed with
//! those ledgers — see the W4/W5 shell, where the open question of whether NT
//! *also* books the 0/1 close (and would thus duplicate this line) is resolved.
//!
//! ## Precondition the caller must honour
//!
//! `settle` takes the YES and NO lots as bare values with no instrument identity,
//! so it cannot verify they belong to the **same** binary market. The shell MUST
//! pair the two lots from one market's two outcome instruments; pairing lots from
//! different markets would sum two unrelated payouts into one booking.
//!
//! Pure: no NautilusTrader type, no async, no I/O. Fail-closed: a lot outside its
//! valid domain — or a non-finite product/sum — yields `None` rather than a
//! poisoned booking. The 0/1 payout values come from [`crate::bolt_v3_numeric`];
//! no inline `0.0`/`1.0` runtime literal on the production path.

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
/// per-share open average price NT records for that instrument.
///
/// Both fields are sourced verbatim from NT's per-instrument accounting
/// (NT-FIRST): `position` is the signed net token quantity, `avg_price` is NT's
/// `avg_px_open`. The notional entry cost is derived as `position × avg_price`
/// inside [`settle`] — never supplied directly — so the cost's sign and unit are
/// structurally correct and cannot be fed wrong.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TokenLot {
    /// Signed token quantity held into expiry: positive = long the token,
    /// negative = short it.
    pub position: f64,
    /// Per-share open average entry price (NT `avg_px_open`). A binary outcome
    /// token is priced as a probability, so this is validated to `[0, 1]`; a value
    /// outside that domain (e.g. a total cash figure, or a sign-flipped price) is
    /// rejected by [`settle`] rather than trusted.
    pub avg_price: f64,
}

impl TokenLot {
    /// Build a lot from NT's signed position quantity and per-share open average
    /// price.
    pub fn new(position: f64, avg_price: f64) -> Self {
        Self {
            position,
            avg_price,
        }
    }

    /// A token the maker holds no position in.
    ///
    /// Provided instead of a `Default` impl: the bolt-v3 legacy-default fence
    /// forbids `Default` on the production surface, and "no lot" is an explicit,
    /// named state — never a silently inherited zeroed value. A flat lot's
    /// `avg_price` is the lower domain bound and contributes no notional cost
    /// (its `position` is zero).
    pub fn flat() -> Self {
        Self {
            position: ZERO_F64,
            avg_price: ZERO_F64,
        }
    }

    /// Whether this lot is in its valid domain: a finite signed position, and a
    /// per-share price that is finite and lives on a binary token's `[0, 1]`
    /// probability domain. This is the enforced cost-basis contract — it rejects a
    /// non-finite, negative, or wrong-unit (`> 1`) price feed.
    fn is_valid(&self) -> bool {
        self.position.is_finite()
            && self.avg_price.is_finite()
            && (ZERO_F64..=UNIT_F64).contains(&self.avg_price)
    }

    /// The derived signed notional entry cost of the lot (`position × avg_price`):
    /// positive cash paid out for a long, negative cash received for a short.
    fn notional_cost(&self) -> f64 {
        self.position * self.avg_price
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
    /// **Gross** realized settlement P&L: terminal `payout` minus the combined
    /// derived notional entry cost of both lots. Gross of fees by design — see the
    /// module docs: commissions are NT-owned, redemption cost shell-owned, rebates
    /// reward-owned, each booked once elsewhere.
    realized_pnl: f64,
}

impl SettlementBooking {
    /// Terminal payout booked for the held inventory at the 0/1 resolution.
    pub fn payout(&self) -> f64 {
        self.payout
    }

    /// Gross realized settlement P&L (`payout − combined notional entry cost`),
    /// exclusive of fees (which are booked by their respective owners).
    pub fn realized_pnl(&self) -> f64 {
        self.realized_pnl
    }
}

/// Settle the maker's held YES and NO token lots at a binary resolution.
///
/// The winning outcome token pays [`UNIT_F64`] per share and the losing token
/// pays [`ZERO_F64`], applied to each lot's signed position independently; the
/// entry cost is derived per lot as `position × avg_price`:
///
/// ```text
/// payout       = yes.position × (UNIT_F64 if YES won else ZERO_F64)
///              + no.position  × (UNIT_F64 if NO  won else ZERO_F64)
/// realized_pnl = payout − (yes.position × yes.avg_price + no.position × no.avg_price)
/// ```
///
/// Because the two lots settle independently, a both-sides inventory books
/// correctly: a long-5-YES + long-5-NO pair redeems its 5 winning tokens for
/// `5 × UNIT_F64` regardless of which side wins, and the P&L is that receipt less
/// the combined entry cost — never a phantom loss from a netted-to-zero position.
///
/// Returns `None` (fail-closed) when either lot is outside its valid domain (a
/// non-finite position, or a per-share `avg_price` that is non-finite or outside
/// `[0, 1]`), or when the resulting `payout`/`realized_pnl` is not finite — a
/// poisoned booking must never silently enter P&L accounting.
pub fn settle(
    yes: TokenLot,
    no: TokenLot,
    outcome: SettlementOutcome,
) -> Option<SettlementBooking> {
    if !yes.is_valid() || !no.is_valid() {
        return None;
    }
    // Each token's terminal per-share value: a unit for the winning token, zero
    // for the losing one. Exactly one outcome wins, so exactly one lot is paid.
    let (yes_token_value, no_token_value) = match outcome {
        Leg::Yes => (UNIT_F64, ZERO_F64),
        Leg::No => (ZERO_F64, UNIT_F64),
    };
    let payout = yes.position * yes_token_value + no.position * no_token_value;
    let notional_cost = yes.notional_cost() + no.notional_cost();
    let realized_pnl = payout - notional_cost;
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
        // The regression the net-equivalent model got wrong: long 5 YES @0.55
        // (cost 2.75) AND long 5 NO @0.45 (cost 2.25) = 5.00, holding a matched
        // pair. Whichever side wins, the 5 winning tokens redeem for 5.00, so
        // realized P&L is ~0 — NOT a 5.00 phantom loss from a netted-to-zero net.
        let yes = TokenLot::new(5.0, 0.55);
        let no = TokenLot::new(5.0, 0.45);
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
        // 10 YES @0.40 (cost 4.0); YES resolves true -> each share pays UNIT_F64.
        let booking =
            settle(TokenLot::new(10.0, 0.40), TokenLot::flat(), Leg::Yes).expect("finite settles");
        assert!((booking.payout() - 10.0).abs() < EPSILON);
        assert!((booking.realized_pnl() - 6.0).abs() < EPSILON); // 10.0 - 4.0
    }

    #[test]
    fn long_yes_pays_zero_when_no_wins_and_books_the_full_cost_as_loss() {
        let booking =
            settle(TokenLot::new(10.0, 0.40), TokenLot::flat(), Leg::No).expect("finite settles");
        assert!(booking.payout().abs() < EPSILON);
        assert!((booking.realized_pnl() - (-4.0)).abs() < EPSILON); // 0 - 4.0
    }

    #[test]
    fn long_no_pays_a_unit_per_share_when_no_wins_and_books_the_gain() {
        // The case the YES-equivalent model booked as a LOSS: 7 NO @ ~0.4286
        // (cost ~3.0); NO resolves true -> the 7 NO tokens redeem for 7.0, +4.0.
        let avg = 3.0 / 7.0;
        let booking =
            settle(TokenLot::flat(), TokenLot::new(7.0, avg), Leg::No).expect("finite settles");
        assert!((booking.payout() - 7.0).abs() < EPSILON);
        assert!((booking.realized_pnl() - 4.0).abs() < EPSILON); // 7.0 - 3.0
    }

    #[test]
    fn long_no_pays_zero_when_yes_wins_and_books_the_full_cost_as_loss() {
        let avg = 3.0 / 7.0;
        let booking =
            settle(TokenLot::flat(), TokenLot::new(7.0, avg), Leg::Yes).expect("finite settles");
        assert!(booking.payout().abs() < EPSILON);
        assert!((booking.realized_pnl() - (-3.0)).abs() < EPSILON); // 0 - 3.0
    }

    #[test]
    fn a_short_winning_token_owes_its_unit_value() {
        // Short 10 YES @0.40 -> derived cost = -10 * 0.40 = -4.0 (4 cash received).
        // YES resolves true: the short owes UNIT_F64 per share.
        let short_yes = TokenLot::new(-10.0, 0.40);
        let booking = settle(short_yes, TokenLot::flat(), Leg::Yes).expect("finite settles");
        assert!((booking.payout() - (-10.0)).abs() < EPSILON);
        // Realized = -10.0 - (-4.0) = -6.0 (sold at 0.40, settles at 1.0).
        assert!((booking.realized_pnl() - (-6.0)).abs() < EPSILON);
        // And when the shorted token loses, the kept cash is the whole gain.
        let won = settle(short_yes, TokenLot::flat(), Leg::No).expect("finite settles");
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
        // Long 8 YES @0.50 (cost 4.0) AND long 3 NO @0.40 (cost 1.2): the two lots
        // settle on their own outcomes, not on a net of 5.
        let yes = TokenLot::new(8.0, 0.50);
        let no = TokenLot::new(3.0, 0.40);
        let yes_wins = settle(yes, no, Leg::Yes).expect("settles");
        assert!((yes_wins.payout() - 8.0).abs() < EPSILON); // 8*1 + 3*0
        assert!((yes_wins.realized_pnl() - (8.0 - 5.2)).abs() < EPSILON);
        let no_wins = settle(yes, no, Leg::No).expect("settles");
        assert!((no_wins.payout() - 3.0).abs() < EPSILON); // 8*0 + 3*1
        assert!((no_wins.realized_pnl() - (3.0 - 5.2)).abs() < EPSILON);
    }

    #[test]
    fn the_payout_minus_combined_notional_identity_holds() {
        // Property: realized_pnl + (yes.notional + no.notional) == payout for any
        // valid lots the module accepts (the booking is internally consistent).
        let positions = [-7.0_f64, ZERO_F64, 0.5, 12.0];
        let prices = [ZERO_F64, 0.25, 0.5, UNIT_F64];
        for &yp in &positions {
            for &ypx in &prices {
                for &np in &positions {
                    for &npx in &prices {
                        for outcome in [Leg::Yes, Leg::No] {
                            let yes = TokenLot::new(yp, ypx);
                            let no = TokenLot::new(np, npx);
                            let booking = settle(yes, no, outcome).expect("valid lots settle");
                            let notional = yp * ypx + np * npx;
                            assert!(
                                (booking.realized_pnl() + notional - booking.payout()).abs()
                                    < EPSILON,
                                "identity broken for yes=({yp},{ypx}) no=({np},{npx}) {outcome:?}",
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_winning_token_pays_a_unit_and_the_losing_token_pays_zero_per_share() {
        // One share on each side at a mid price: exactly UNIT_F64 from the winner.
        let one_each = (TokenLot::new(UNIT_F64, 0.5), TokenLot::new(UNIT_F64, 0.5));
        let yes_won = settle(one_each.0, one_each.1, Leg::Yes).expect("settles");
        assert!((yes_won.payout() - UNIT_F64).abs() < EPSILON);
        let no_won = settle(one_each.0, one_each.1, Leg::No).expect("settles");
        assert!((no_won.payout() - UNIT_F64).abs() < EPSILON);
    }

    #[test]
    fn a_non_finite_field_fails_closed_to_none() {
        let live = TokenLot::new(10.0, 0.40);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(settle(TokenLot::new(bad, 0.40), live, Leg::Yes).is_none());
            assert!(settle(TokenLot::new(10.0, bad), live, Leg::Yes).is_none());
            assert!(settle(live, TokenLot::new(bad, 0.40), Leg::No).is_none());
            assert!(settle(live, TokenLot::new(1.0, bad), Leg::No).is_none());
        }
    }

    #[test]
    fn a_price_outside_the_binary_token_domain_fails_closed() {
        // The exact contract attack from the adversarial review: a TOTAL cash
        // figure (4.0) fed where a per-share price belongs is > 1 -> rejected,
        // rather than silently booking a wrong cost basis.
        assert!(settle(TokenLot::new(10.0, 4.0), TokenLot::flat(), Leg::Yes).is_none());
        // A sign-flipped (negative) price is not a valid token price -> rejected.
        assert!(settle(TokenLot::new(10.0, -0.40), TokenLot::flat(), Leg::Yes).is_none());
        // Just past the upper bound is rejected; exactly at the bounds is allowed.
        assert!(settle(TokenLot::new(1.0, 1.000_001), TokenLot::flat(), Leg::Yes).is_none());
        assert!(settle(TokenLot::new(1.0, UNIT_F64), TokenLot::flat(), Leg::Yes).is_some());
        assert!(settle(TokenLot::new(1.0, ZERO_F64), TokenLot::flat(), Leg::Yes).is_some());
    }
}
