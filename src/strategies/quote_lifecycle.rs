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
//! arrives.
//!
//! Scope (W2 slice 1): a single leg on the cancel+resubmit requote path — the
//! Polymarket binary path, whose adapter has no order-modify. The modify-capable
//! branch (driven by the venue capability contract), the second YES/NO leg,
//! cancel-scope, the requote throttle, reconnect resync, and the NT handler
//! translation arrive in later W2 slices.

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
    /// The venue accepted the in-flight order.
    Accepted,
    /// The venue rejected the in-flight order.
    Rejected,
    /// The venue confirmed the in-flight cancel.
    Canceled,
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
}

/// A single quote leg and its lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuoteLeg {
    state: LegState,
}

impl QuoteLeg {
    /// A fresh leg with no resting order. Constructed explicitly (no `Default`):
    /// the bolt-v3 legacy-default fence forbids a `Default` impl on the
    /// production surface, so callers must name the starting state.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            state: LegState::Idle,
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
    /// `QuoteTrigger` while a submit or cancel is already in flight emits nothing,
    /// so a leg can never have two commands outstanding at once.
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
            // T4 (cancel+resubmit): a resting quote whose price has moved beyond
            // the requote threshold is cancelled first; the replacement submit is
            // emitted only once the cancel confirms (T5).
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
            // Everything else is a no-op: a no-move trigger while Resting, any
            // trigger while a command is in flight, or an accept/reject/cancel
            // event that does not match the current state.
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_trigger_submits_and_pends() {
        let mut leg = QuoteLeg::new();
        assert_eq!(leg.state(), LegState::Idle);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        assert_eq!(action, Some(LifecycleAction::Submit));
        assert_eq!(leg.state(), LegState::SubmitPending);
    }

    #[test]
    fn submit_pending_accepted_rests() {
        let mut leg = QuoteLeg::new();
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        let action = leg.on_event(LegEvent::Accepted);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::Resting);
    }

    #[test]
    fn submit_pending_rejected_returns_to_idle() {
        let mut leg = QuoteLeg::new();
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        let action = leg.on_event(LegEvent::Rejected);
        assert_eq!(action, None);
        assert_eq!(leg.state(), LegState::Idle);
    }

    #[test]
    fn resting_requote_cancels_and_pends() {
        let mut leg = QuoteLeg::new();
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        leg.on_event(LegEvent::Accepted);
        assert_eq!(leg.state(), LegState::Resting);
        let action = leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(action, Some(LifecycleAction::Cancel));
        assert_eq!(leg.state(), LegState::RequotePending);
    }

    #[test]
    fn requote_pending_canceled_resubmits() {
        let mut leg = QuoteLeg::new();
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        leg.on_event(LegEvent::Accepted);
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: true,
        });
        assert_eq!(leg.state(), LegState::RequotePending);
        let action = leg.on_event(LegEvent::Canceled);
        assert_eq!(action, Some(LifecycleAction::Submit));
        assert_eq!(leg.state(), LegState::SubmitPending);
    }

    #[test]
    fn second_trigger_while_in_flight_emits_no_duplicate_command() {
        let mut leg = QuoteLeg::new();
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: false
            }),
            Some(LifecycleAction::Submit)
        );
        // SubmitPending: a second trigger must not emit a duplicate submit.
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: true
            }),
            None
        );
        assert_eq!(leg.state(), LegState::SubmitPending);

        // RequotePending: same single-command-in-flight guarantee.
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
    fn resting_no_move_trigger_is_a_noop() {
        let mut leg = QuoteLeg::new();
        leg.on_event(LegEvent::QuoteTrigger {
            requote_needed: false,
        });
        leg.on_event(LegEvent::Accepted);
        assert_eq!(leg.state(), LegState::Resting);
        assert_eq!(
            leg.on_event(LegEvent::QuoteTrigger {
                requote_needed: false
            }),
            None
        );
        assert_eq!(leg.state(), LegState::Resting);
    }
}
