//! Pure, NautilusTrader-free single-leg quote-lifecycle state machine for the
//! binary-oracle maker (W2 — Quote Lifecycle / Execution Control).
//!
//! It models one resting-quote leg as a [`LegState`] plus a transition function
//! ([`QuoteLeg::on_event`]) that consumes [`LegEvent`]s and emits
//! [`LifecycleAction`]s the strategy layer translates into NT order calls.
//!
//! The machine deliberately holds no NautilusTrader type: NT remains the single
//! owner of order submission/cancellation/fills (NT-FIRST, NO DUAL PATHS), and
//! this module only names the *intent*. Keeping it pure makes the lifecycle
//! exhaustively unit-testable without a runtime, and lets the same machine be
//! reused unchanged behind the execution-adapter seam when a second venue
//! arrives. It is venue-agnostic by construction: the requote path is selected
//! by the `supports_modify` capability fact (a `bool` sourced from the venue
//! contract), never by a venue name.
//!
//! Scope (W2 slices 1–2): a single leg on both requote paths — cancel+resubmit
//! (Polymarket binary; the adapter has no order-modify) and modify-in-place
//! (modify-capable venues), with a modify-reject degrade back to cancel+resubmit.
//! The second YES/NO leg, cancel-scope, the requote throttle, reconnect resync,
//! and the NT handler translation arrive in later W2 slices.

/// Lifecycle state of a single quote leg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegState {
    /// No resting order and nothing in flight.
    Idle,
    /// A submit has been emitted; awaiting the venue's accept/reject.
    SubmitPending,
    /// A live resting quote is on the book.
    Resting,
    /// A requote cancel has been emitted; awaiting the cancel confirmation
    /// before the replacement submit (cancel+resubmit venues).
    RequotePending,
    /// An in-place modify has been emitted; awaiting the modify confirmation
    /// (modify-capable venues). A modify reject degrades to cancel+resubmit.
    ModifyPending,
}

/// Events that drive a leg.
///
/// The caller derives `requote_needed` by comparing the freshly-computed quote
/// price against the resting price using the configured requote threshold; the
/// pure machine never computes prices itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegEvent {
    /// A pricing tick. `requote_needed` is true when the desired quote has moved
    /// beyond the requote threshold from the currently resting one.
    QuoteTrigger { requote_needed: bool },
    /// The venue accepted the in-flight submit.
    Accepted,
    /// The venue rejected the in-flight submit.
    Rejected,
    /// The venue confirmed the in-flight cancel.
    Canceled,
    /// The venue confirmed the in-flight in-place modify.
    Modified,
    /// The venue rejected the in-flight in-place modify.
    ModifyRejected,
}

/// The order intent the strategy layer must execute against NautilusTrader.
///
/// The pure machine never calls NT; it only emits this intent, which the maker's
/// NT event handlers (a later slice) translate into the corresponding NT call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    /// Submit a fresh post-only limit quote for this leg.
    Submit,
    /// Cancel the resting quote (the cancel+resubmit requote path).
    Cancel,
    /// Modify the resting quote in place (modify-capable venues).
    Modify,
}

/// A single quote leg, its lifecycle state, and the one venue-capability fact the
/// requote path depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteLeg {
    state: LegState,
    /// Whether the venue supports in-place order modification (from the venue
    /// capability contract). When false, a requote cancels then resubmits.
    supports_modify: bool,
}

impl QuoteLeg {
    /// A fresh leg with no resting order. Constructed explicitly (no `Default`):
    /// the bolt-v3 legacy-default fence forbids a `Default` impl on the
    /// production surface, so callers must name the starting state and pass the
    /// `supports_modify` venue-capability fact.
    pub fn new(supports_modify: bool) -> Self {
        Self {
            state: LegState::Idle,
            supports_modify,
        }
    }

    /// The leg's current lifecycle state.
    pub fn state(&self) -> LegState {
        self.state
    }

    /// Drive the leg with one event, advancing its state and returning the order
    /// intent to execute (if any).
    ///
    /// Fail-closed by construction: an event that does not apply to the current
    /// state is a no-op (no action, no state change). In particular a second
    /// `QuoteTrigger` while a submit, cancel, or modify is already in flight emits
    /// nothing, so a leg can never have two commands outstanding at once.
    pub fn on_event(&mut self, event: LegEvent) -> Option<LifecycleAction> {
        match (self.state, event) {
            // T1: Idle + trigger -> submit a fresh quote.
            (LegState::Idle, LegEvent::QuoteTrigger { .. }) => {
                self.state = LegState::SubmitPending;
                Some(LifecycleAction::Submit)
            }
            // T2: in-flight submit accepted -> the quote is resting.
            (LegState::SubmitPending, LegEvent::Accepted) => {
                self.state = LegState::Resting;
                None
            }
            // T3: in-flight submit rejected -> drop the leg. The governor decides
            // whether/when to re-quote; there is no automatic resubmit here.
            (LegState::SubmitPending, LegEvent::Rejected) => {
                self.state = LegState::Idle;
                None
            }
            // T4-modify: a resting quote that has moved, on a modify-capable
            // venue, is amended in place — one Modify, no cancel/resubmit cycle.
            (
                LegState::Resting,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            ) if self.supports_modify => {
                self.state = LegState::ModifyPending;
                Some(LifecycleAction::Modify)
            }
            // T4-cancel: same trigger on a modify-unsupported venue (Polymarket)
            // cancels first; the replacement submit is emitted once the cancel
            // confirms (T5).
            (
                LegState::Resting,
                LegEvent::QuoteTrigger {
                    requote_needed: true,
                },
            ) => {
                self.state = LegState::RequotePending;
                Some(LifecycleAction::Cancel)
            }
            // T5: the requote cancel confirmed -> submit the replacement quote.
            (LegState::RequotePending, LegEvent::Canceled) => {
                self.state = LegState::SubmitPending;
                Some(LifecycleAction::Submit)
            }
            // T5-modify: the in-place modify confirmed -> the quote rests again at
            // the new price; no resubmit needed.
            (LegState::ModifyPending, LegEvent::Modified) => {
                self.state = LegState::Resting;
                None
            }
            // T6: the modify was rejected -> degrade to cancel+resubmit (cancel
            // now, resubmit on the cancel confirmation via T5).
            (LegState::ModifyPending, LegEvent::ModifyRejected) => {
                self.state = LegState::RequotePending;
                Some(LifecycleAction::Cancel)
            }
            // Everything else is a no-op: a no-move trigger while Resting, any
            // trigger while a command is in flight, or an event that does not
            // match the current state.
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a leg that already holds a resting quote, for the requote tests.
    fn resting_leg(supports_modify: bool) -> QuoteLeg {
        let mut leg = QuoteLeg::new(supports_modify);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        leg.on_event(LegEvent::Accepted);
        assert_eq!(leg.state(), LegState::Resting);
        leg
    }

    #[test]
    fn idle_trigger_submits_and_pends() {
        let mut leg = QuoteLeg::new(false);
        assert_eq!(leg.state(), LegState::Idle);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        assert_eq!(action, Some(LifecycleAction::Submit));
        assert_eq!(leg.state(), LegState::SubmitPending);
    }

    #[test]
    fn submit_pending_accepted_rests() {
        let mut leg = QuoteLeg::new(false);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        let action = leg.on_event(LegEvent::Accepted);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::Resting);
    }

    #[test]
    fn submit_pending_rejected_returns_to_idle() {
        let mut leg = QuoteLeg::new(false);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        let action = leg.on_event(LegEvent::Rejected);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn cancel_resubmit_requote_when_modify_unsupported() {
        let mut leg = resting_leg(false);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(action, Some(LifecycleAction::Cancel));
        assert_eq!(leg.state(), LegState::RequotePending);
        // The replacement submit is emitted only when the cancel confirms.
        let action = leg.on_event(LegEvent::Canceled);
        assert_eq!(action, Some(LifecycleAction::Submit));
        assert_eq!(leg.state(), LegState::SubmitPending);
    }

    #[test]
    fn modify_in_place_requote_when_modify_supported() {
        let mut leg = resting_leg(true);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        // One Modify, NOT a Cancel.
        assert_eq!(action, Some(LifecycleAction::Modify));
        assert_eq!(leg.state(), LegState::ModifyPending);
        // The modify confirmation returns the leg to Resting with no resubmit.
        let action = leg.on_event(LegEvent::Modified);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::Resting);
    }

    #[test]
    fn modify_reject_degrades_to_cancel_resubmit() {
        let mut leg = resting_leg(true);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::ModifyPending);
        // A modify reject degrades to cancel+resubmit.
        let action = leg.on_event(LegEvent::ModifyRejected);
        assert_eq!(action, Some(LifecycleAction::Cancel));
        assert_eq!(leg.state(), LegState::RequotePending);
        let action = leg.on_event(LegEvent::Canceled);
        assert_eq!(action, Some(LifecycleAction::Submit));
        assert_eq!(leg.state(), LegState::SubmitPending);
    }

    #[test]
    fn second_trigger_while_in_flight_emits_no_duplicate_command() {
        // Cancel+resubmit path.
        let mut leg = QuoteLeg::new(false);
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: false
            }),
            Some(LifecycleAction::Submit)
        );
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: true
            }),
            None
        );
        assert_eq!(leg.state(), LegState::SubmitPending);
        leg.on_event(LegEvent::Accepted);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::RequotePending);
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: true
            }),
            None
        );
        assert_eq!(leg.state(), LegState::RequotePending);
    }

    #[test]
    fn second_trigger_while_modify_in_flight_emits_no_duplicate_command() {
        let mut leg = resting_leg(true);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::ModifyPending);
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: true
            }),
            None
        );
        assert_eq!(leg.state(), LegState::ModifyPending);
    }

    #[test]
    fn resting_no_move_trigger_is_a_noop() {
        for supports_modify in [false, true] {
            let mut leg = resting_leg(supports_modify);
            assert_eq!(
                leg.on_event(LegEvent::QuoteTrigger {
                    requote_needed: false
                }),
                None
            );
            assert_eq!(leg.state(), LegState::Resting);
        }
    }
}
