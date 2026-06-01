//! Pure, NautilusTrader-free reconnect reconciliation and cancel-all-on-kill for
//! the binary-oracle maker (W2 completion — SC-003: no orphaned/duplicate quotes
//! across reprice, kill, and reconnect).
//!
//! Two responsibilities, both expressed entirely in the existing
//! [`crate::strategies::quote_lifecycle`] vocabulary ([`MarketAction`] /
//! [`LifecycleAction`]) so there is no parallel intent language (NO DUAL PATHS):
//!
//! 1. **Reconnect reconciliation** (spec line 46 / SC-003). After a disconnect
//!    the maker's local belief about which quotes are resting can diverge from
//!    the venue's accepted-order truth. [`MarketReconcileSnapshot`] pairs, per
//!    leg, what the maker *believes* is resting against what the venue *reports*
//!    open once the session resumes; [`MarketReconcileSnapshot::reconcile`]
//!    produces the intents that re-converge the two truths before any new
//!    quoting: CANCEL venue orders the maker does not recognise (orphans),
//!    RE-ESTABLISH legs the maker believed resting but the venue does not report,
//!    and ADOPT (no action) the matches. It is a pure function of the snapshot,
//!    so re-running it on an already-reconciled snapshot — only matched or
//!    mutually-absent legs — yields no actions (idempotent).
//!
//! 2. **Cancel-all-on-kill** (spec line 45 / SC-003). [`cancel_all_on_kill`] maps
//!    a [`MakerGovernorState`] to a cancel-BOTH-legs intent via
//!    [`MarketQuote::drain`], so a kill pulls *every* resting quote on both legs,
//!    not a single pending order. Only the terminal-pull postures
//!    (`HardFlat(_)` and `CancelOnly`) trigger it; `Quoting`, `ReduceOnly`, and
//!    `SoftHold` keep (some of) their quotes and so never cancel-all (see the
//!    mapping table on the function).
//!
//! The module names only *intent*: NT remains the single owner of order
//! submission/cancellation (NT-FIRST), and the strategy shell translates each
//! emitted [`MarketAction`] into the matching NT call and applies the resulting
//! lifecycle transition. Keeping the reconciliation pure makes the orphan /
//! missing / matched matrix exhaustively unit-testable without a runtime.

use crate::strategies::maker_governor::MakerGovernorState;
use crate::strategies::quote_lifecycle::{Leg, LifecycleAction, MarketAction, MarketQuote};

/// Per-leg reconnect snapshot: the maker's local belief versus the venue's
/// reported open-order truth for one side of the binary market, captured once
/// the session resumes.
///
/// Both facts are booleans because the reconciliation decides *liveness*, not
/// price: a leg either has a recognised resting quote or it does not, and the
/// venue either reports an open order on that side or it does not. Price-level
/// re-pricing is the governor/maker model's job on the subsequent tick, after
/// the local and venue truths agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegReconcileSnapshot {
    /// Which side this snapshot describes.
    pub leg: Leg,
    /// The maker believes a quote it owns is resting on this leg (its local
    /// lifecycle state holds a `Resting`/in-flight order for this side).
    pub believed_resting: bool,
    /// The venue reports an open order on this side after the reconnect — the
    /// accepted-order truth the local belief must be reconciled against.
    pub venue_reports_open: bool,
}

impl LegReconcileSnapshot {
    /// Build a per-leg snapshot from the two reconnect truths.
    pub fn new(leg: Leg, believed_resting: bool, venue_reports_open: bool) -> Self {
        Self {
            leg,
            believed_resting,
            venue_reports_open,
        }
    }

    /// The single reconcile intent for this leg (if any).
    ///
    /// The four-cell matrix, fail-closed toward never leaving an untracked live
    /// order and never assuming a quote rests that the venue does not confirm:
    ///
    /// | believed | venue open | outcome                                   |
    /// |----------|-----------|--------------------------------------------|
    /// | true     | true      | ADOPT — match; no action                   |
    /// | false    | true      | ORPHAN — CANCEL the unrecognised venue order|
    /// | true     | false     | MISSING — RE-ESTABLISH (submit) the leg     |
    /// | false    | false     | nothing — no order either side; no action  |
    ///
    /// ADOPT and the mutually-absent cell emit nothing, which is exactly what
    /// makes [`MarketReconcileSnapshot::reconcile`] idempotent on an
    /// already-reconciled snapshot.
    fn reconcile_intent(self) -> Option<MarketAction> {
        match (self.believed_resting, self.venue_reports_open) {
            // ADOPT: the maker's resting quote is exactly the venue's open order.
            // Already converged — no action (and the no-op that keeps re-resync
            // idempotent).
            (true, true) => None,
            // ORPHAN: the venue holds an order the maker does not recognise (e.g.
            // an accept that landed during the disconnect). Cancel it rather than
            // leave an untracked live quote — the SC-003 orphan hazard.
            (false, true) => Some(MarketAction::Leg {
                leg: self.leg,
                action: LifecycleAction::Cancel,
            }),
            // MISSING: the maker believed a quote rested, but the venue reports
            // none (it was cancelled/filled while disconnected). Re-establish a
            // fresh quote rather than continue believing in a quote that is gone.
            (true, false) => Some(MarketAction::Leg {
                leg: self.leg,
                action: LifecycleAction::Submit,
            }),
            // Neither side has an order: nothing to reconcile.
            (false, false) => None,
        }
    }
}

/// A market's two per-leg reconnect snapshots (YES and NO), the unit the
/// reconciliation operates over.
///
/// Holding both legs explicitly (rather than a variable-length collection)
/// mirrors [`MarketQuote`]'s fixed two-leg shape and guarantees the
/// reconciliation visits each side exactly once in a deterministic order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarketReconcileSnapshot {
    yes: LegReconcileSnapshot,
    no: LegReconcileSnapshot,
}

impl MarketReconcileSnapshot {
    /// Build the market snapshot from its two leg snapshots.
    ///
    /// Fail-closed: returns `None` if the snapshots do not cover exactly the YES
    /// and NO sides (both same side, or a side missing). A mislabelled snapshot
    /// pair would silently reconcile one side twice and skip the other —
    /// rejecting it up front prevents reconciling against a malformed view.
    pub fn new(yes: LegReconcileSnapshot, no: LegReconcileSnapshot) -> Option<Self> {
        if yes.leg != Leg::Yes || no.leg != Leg::No {
            return None;
        }
        Some(Self { yes, no })
    }

    /// Produce the reconcile intents that re-converge local belief and venue
    /// truth, in deterministic YES-then-NO order.
    ///
    /// Each emitted [`MarketAction`] is a per-leg CANCEL (orphan) or SUBMIT
    /// (re-establish); ADOPT and mutually-absent legs emit nothing. Re-running
    /// `reconcile` on a snapshot that describes an already-reconciled market
    /// (every leg matched or mutually absent) returns an empty plan — the
    /// idempotence SC-003 requires across repeated reconnect handling.
    ///
    /// The caller applies these intents through the same NT-call/lifecycle path
    /// as any other [`MarketAction`] before resuming new quoting (spec line 46:
    /// reconcile against accepted-order truth *before* new quoting).
    pub fn reconcile(self) -> Vec<MarketAction> {
        [self.yes, self.no]
            .into_iter()
            .filter_map(LegReconcileSnapshot::reconcile_intent)
            .collect()
    }
}

/// Map a governor posture to the cancel-all-on-kill intent (spec line 45 /
/// SC-003): a kill cancels **both** legs, not a single pending order.
///
/// Reuses [`MarketQuote::drain`] — one `cancel_all_orders(instrument, None)`
/// over both legs — rather than inventing a parallel cancel path. The posture
/// mapping:
///
/// | posture            | cancel-all? | why                                     |
/// |--------------------|-------------|-----------------------------------------|
/// | `HardFlat(_)`      | yes (drain) | a kill predicate tripped — pull both legs|
/// | `CancelOnly`       | yes (drain) | ops pull (maintenance / reconnect) — pull|
/// | `ReduceOnly`       | no          | still quotes the inventory-reducing side |
/// | `SoftHold`         | no          | keeps minimal quotes for reward eligibility|
/// | `Quoting`          | no          | normal two-sided quoting                 |
///
/// Returns `None` for the non-kill postures and for a kill over a market with no
/// working orders (nothing to cancel) — `drain` itself emits nothing when both
/// legs are already idle, keeping the call idempotent under a repeated kill.
pub fn cancel_all_on_kill(
    state: MakerGovernorState,
    market: &mut MarketQuote,
) -> Option<MarketAction> {
    match state {
        // Terminal pulls: cancel every resting quote on both legs.
        MakerGovernorState::HardFlat(_) | MakerGovernorState::CancelOnly => market.drain(),
        // Postures that keep (some of) their quotes never cancel-all.
        MakerGovernorState::Quoting
        | MakerGovernorState::ReduceOnly
        | MakerGovernorState::SoftHold => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategies::maker_governor::KillReason;
    use crate::strategies::quote_lifecycle::LegState;

    /// A market with both legs resting, for the kill-mapping tests.
    fn resting_market() -> MarketQuote {
        let mut market = MarketQuote::new(false);
        for leg in [Leg::Yes, Leg::No] {
            market.on_leg_event(
                leg,
                crate::strategies::quote_lifecycle::LegEvent::QuoteTrigger {
                    requote_needed: false,
                },
            );
            market.on_leg_event(leg, crate::strategies::quote_lifecycle::LegEvent::Accepted);
            assert_eq!(market.leg_state(leg), LegState::Resting);
        }
        market
    }

    /// Build a valid YES/NO market snapshot from the four reconnect truths.
    fn snapshot(
        yes_believed: bool,
        yes_open: bool,
        no_believed: bool,
        no_open: bool,
    ) -> MarketReconcileSnapshot {
        MarketReconcileSnapshot::new(
            LegReconcileSnapshot::new(Leg::Yes, yes_believed, yes_open),
            LegReconcileSnapshot::new(Leg::No, no_believed, no_open),
        )
        .expect("well-formed YES/NO snapshot")
    }

    // --- reconnect reconciliation: the four-cell matrix per leg ---

    #[test]
    fn orphan_venue_order_is_cancelled() {
        // The maker does not recognise it; the venue reports it open -> CANCEL.
        let plan = snapshot(false, true, false, false).reconcile();
        assert_eq!(
            plan,
            vec![MarketAction::Leg {
                leg: Leg::Yes,
                action: LifecycleAction::Cancel,
            }]
        );
    }

    #[test]
    fn believed_resting_but_venue_absent_is_re_established() {
        // The maker believed a quote rested; the venue reports none -> SUBMIT.
        let plan = snapshot(false, false, true, false).reconcile();
        assert_eq!(
            plan,
            vec![MarketAction::Leg {
                leg: Leg::No,
                action: LifecycleAction::Submit,
            }]
        );
    }

    #[test]
    fn matched_leg_is_adopted_with_no_action() {
        // Belief and venue agree on both legs -> ADOPT, no intents.
        let plan = snapshot(true, true, true, true).reconcile();
        assert!(
            plan.is_empty(),
            "a fully-matched market reconciles to no-op"
        );
    }

    #[test]
    fn mutually_absent_leg_emits_nothing() {
        // No belief and no venue order on either side -> nothing to reconcile.
        let plan = snapshot(false, false, false, false).reconcile();
        assert!(plan.is_empty());
    }

    #[test]
    fn mixed_legs_emit_each_sides_intent_in_yes_then_no_order() {
        // YES is an orphan (cancel); NO is missing (re-establish). Deterministic
        // YES-before-NO order.
        let plan = snapshot(false, true, true, false).reconcile();
        assert_eq!(
            plan,
            vec![
                MarketAction::Leg {
                    leg: Leg::Yes,
                    action: LifecycleAction::Cancel,
                },
                MarketAction::Leg {
                    leg: Leg::No,
                    action: LifecycleAction::Submit,
                },
            ]
        );
    }

    #[test]
    fn re_resync_on_reconciled_snapshot_is_idempotent() {
        // After a reconcile the shell adopts matches and cancels/re-submits the
        // rest; a fresh snapshot of the converged state (matched or mutually
        // absent legs only) must yield no further actions on any re-run.
        for (yes_b, yes_o, no_b, no_o) in [
            (true, true, true, true),     // both matched
            (true, true, false, false),   // YES matched, NO mutually absent
            (false, false, true, true),   // YES mutually absent, NO matched
            (false, false, false, false), // both mutually absent
        ] {
            let snap = snapshot(yes_b, yes_o, no_b, no_o);
            assert!(snap.reconcile().is_empty(), "converged snapshot is a no-op");
            // Re-running on the SAME snapshot is still empty (pure function of
            // the snapshot — no hidden state to drift between runs).
            assert!(
                snap.reconcile().is_empty(),
                "second reconcile stays a no-op"
            );
        }
    }

    #[test]
    fn both_orphans_cancel_both_sides() {
        let plan = snapshot(false, true, false, true).reconcile();
        assert_eq!(
            plan,
            vec![
                MarketAction::Leg {
                    leg: Leg::Yes,
                    action: LifecycleAction::Cancel,
                },
                MarketAction::Leg {
                    leg: Leg::No,
                    action: LifecycleAction::Cancel,
                },
            ]
        );
    }

    #[test]
    fn both_missing_re_establish_both_sides() {
        let plan = snapshot(true, false, true, false).reconcile();
        assert_eq!(
            plan,
            vec![
                MarketAction::Leg {
                    leg: Leg::Yes,
                    action: LifecycleAction::Submit,
                },
                MarketAction::Leg {
                    leg: Leg::No,
                    action: LifecycleAction::Submit,
                },
            ]
        );
    }

    #[test]
    fn snapshot_rejects_mislabelled_or_duplicate_legs() {
        // Both YES: would reconcile one side twice and skip NO -> rejected.
        assert!(
            MarketReconcileSnapshot::new(
                LegReconcileSnapshot::new(Leg::Yes, false, true),
                LegReconcileSnapshot::new(Leg::Yes, false, true),
            )
            .is_none()
        );
        // Swapped order (NO in the YES slot) -> rejected.
        assert!(
            MarketReconcileSnapshot::new(
                LegReconcileSnapshot::new(Leg::No, false, false),
                LegReconcileSnapshot::new(Leg::Yes, false, false),
            )
            .is_none()
        );
        // Correct YES/NO labelling -> accepted.
        assert!(
            MarketReconcileSnapshot::new(
                LegReconcileSnapshot::new(Leg::Yes, true, true),
                LegReconcileSnapshot::new(Leg::No, true, true),
            )
            .is_some()
        );
    }

    // --- cancel-all-on-kill: posture mapping ---

    #[test]
    fn hard_flat_cancels_both_legs() {
        for reason in [
            KillReason::TauFloor,
            KillReason::SigmaFloor,
            KillReason::BasisCap,
        ] {
            let mut market = resting_market();
            let action = cancel_all_on_kill(MakerGovernorState::HardFlat(reason), &mut market);
            assert_eq!(action, Some(MarketAction::CancelAllBothLegs));
            // BOTH legs are now winding down — not a single pending order.
            assert_eq!(market.leg_state(Leg::Yes), LegState::CancelPending);
            assert_eq!(market.leg_state(Leg::No), LegState::CancelPending);
        }
    }

    #[test]
    fn cancel_only_cancels_both_legs() {
        let mut market = resting_market();
        let action = cancel_all_on_kill(MakerGovernorState::CancelOnly, &mut market);
        assert_eq!(action, Some(MarketAction::CancelAllBothLegs));
        assert_eq!(market.leg_state(Leg::Yes), LegState::CancelPending);
        assert_eq!(market.leg_state(Leg::No), LegState::CancelPending);
    }

    #[test]
    fn non_kill_postures_never_cancel_all() {
        for state in [
            MakerGovernorState::Quoting,
            MakerGovernorState::ReduceOnly,
            MakerGovernorState::SoftHold,
        ] {
            let mut market = resting_market();
            assert_eq!(cancel_all_on_kill(state, &mut market), None);
            // Both legs stay resting — nothing is pulled.
            assert_eq!(market.leg_state(Leg::Yes), LegState::Resting);
            assert_eq!(market.leg_state(Leg::No), LegState::Resting);
        }
    }

    #[test]
    fn kill_over_an_idle_market_emits_nothing() {
        // A kill when no quotes rest is a no-op (drain emits nothing), so a
        // repeated kill after the first drain stays idempotent.
        let mut market = MarketQuote::new(false);
        assert_eq!(
            cancel_all_on_kill(
                MakerGovernorState::HardFlat(KillReason::TauFloor),
                &mut market
            ),
            None
        );
        // And re-killing an already-draining market emits nothing further.
        let mut draining = resting_market();
        assert_eq!(
            cancel_all_on_kill(MakerGovernorState::CancelOnly, &mut draining),
            Some(MarketAction::CancelAllBothLegs)
        );
        assert_eq!(
            cancel_all_on_kill(MakerGovernorState::CancelOnly, &mut draining),
            None
        );
    }
}
