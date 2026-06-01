//! Pure, NautilusTrader-free venue-event fence for the binary-oracle maker
//! (W2 — NT-handler acceptance criterion, "Invariant B"; SC-003).
//!
//! The pure lifecycle machine ([`crate::strategies::quote_lifecycle::QuoteLeg`])
//! is fail-closed *only if* every venue report it consumes belongs to the order
//! that leg currently expects. On a cancel+resubmit venue (Polymarket binary has
//! no order-modify) a leg requotes by cancelling one order and submitting a
//! fresh one, so the wire briefly carries reports for both the old and the new
//! order. Feeding a stale or misrouted `Accepted`/`Canceled`/… into the machine
//! would misapply it to the successor order — e.g. a late `Canceled` for the
//! *previous* quote would tear down the *new* resting quote. This module is the
//! fence that prevents that: it matches each raw venue report to the leg's
//! current expected order **before** any [`LegEvent`] reaches the machine, and
//! rejects (fail-closed) anything that does not match.
//!
//! Identity is agnostic by construction. A maker order is named by an
//! [`OrderIdentity`]: a [`ClientOrderId`] (a newtype over `String`, not a
//! venue-specific id format — the NT shell maps NT's own `ClientOrderId` to and
//! from this) plus a `generation` — a per-leg monotonic counter the maker bumps
//! on every requote ([`ExpectedIdentity::requote_to`]). The generation is what
//! distinguishes a fresh order from the stale one it replaced even when an
//! adapter reuses or recycles a client id, so a new order can never inherit its
//! predecessor's in-flight events.
//!
//! Pure: no NautilusTrader type, no I/O, no clock, no randomness. The NT event
//! handler (the shell, a later slice) owns extracting `(client_order_id,
//! generation, kind)` from each NT execution report and routing the returned
//! `LegEvent` into the per-leg machine; this module owns only the match/reject
//! decision, so it is exhaustively unit-testable without a runtime.

use crate::strategies::quote_lifecycle::LegEvent;

/// An agnostic client order id — a newtype over `String`, deliberately not a
/// venue-specific id format. The NT shell maps NautilusTrader's own
/// `ClientOrderId` into this and back; keeping the fence over a plain `String`
/// is what keeps it free of any NT or venue type (NO DUAL PATHS: one identity
/// vocabulary, owned here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientOrderId(String);

impl ClientOrderId {
    /// Wrap a raw client-order-id string.
    pub fn new(id: String) -> Self {
        Self(id)
    }

    /// The underlying id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The agnostic identity of one maker order: its [`ClientOrderId`] plus the
/// `generation` the leg was on when the order was submitted.
///
/// `generation` is a per-leg monotonic counter the maker bumps each requote
/// (via [`ExpectedIdentity::requote_to`]). It exists so a fresh order never
/// inherits the in-flight venue events of the stale order it replaced: two
/// orders that happen to share a client id but were submitted on different
/// generations are distinct identities here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderIdentity {
    client_order_id: ClientOrderId,
    generation: u64,
}

impl OrderIdentity {
    /// Name an order by its client id and the generation it was submitted on.
    pub fn new(client_order_id: ClientOrderId, generation: u64) -> Self {
        Self {
            client_order_id,
            generation,
        }
    }

    /// The order's client id.
    pub fn client_order_id(&self) -> &ClientOrderId {
        &self.client_order_id
    }

    /// The generation the order was submitted on.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

/// A raw, untrusted venue execution report as the NT shell extracted it: the
/// order it claims to be about (`client_order_id` + `generation`) and what kind
/// of report it is. The fence ([`ExpectedIdentity::admit`]) decides whether to
/// trust it.
///
/// `kind` is deliberately only the venue-originated subset of lifecycle events:
/// [`LegEvent::QuoteTrigger`] is a *local* pricing decision the strategy raises
/// itself, never a venue report, so it has no [`VenueReportKind`] and can never
/// arrive through this fence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VenueReport {
    /// The client id the venue report is tagged with.
    pub client_order_id: ClientOrderId,
    /// The generation the reported order was submitted on (carried through by
    /// the shell from the order's [`OrderIdentity`] at submission time).
    pub generation: u64,
    /// What the venue is reporting.
    pub kind: VenueReportKind,
}

/// The kinds of report a venue emits about a resting order, one per
/// venue-originated [`LegEvent`]. Mapped 1:1 to a `LegEvent` by
/// [`VenueReportKind::into_leg_event`] once identity is confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VenueReportKind {
    /// The venue accepted the in-flight submit. -> [`LegEvent::Accepted`].
    Accepted,
    /// The venue rejected the in-flight submit. -> [`LegEvent::Rejected`].
    Rejected,
    /// The venue confirmed the in-flight cancel. -> [`LegEvent::Canceled`].
    Canceled,
    /// The venue confirmed the in-flight in-place modify. -> [`LegEvent::Modified`].
    Modified,
    /// The venue rejected the in-flight in-place modify. -> [`LegEvent::ModifyRejected`].
    ModifyRejected,
    /// The venue rejected the in-flight cancel. -> [`LegEvent::CancelRejected`].
    CancelRejected,
    /// The order fully filled and left the book. -> [`LegEvent::Filled`].
    Filled,
}

impl VenueReportKind {
    /// Map this venue report kind to the lifecycle event it drives. Total: every
    /// kind has exactly one event, and no kind maps to the local-only
    /// [`LegEvent::QuoteTrigger`].
    fn into_leg_event(self) -> LegEvent {
        match self {
            VenueReportKind::Accepted => LegEvent::Accepted,
            VenueReportKind::Rejected => LegEvent::Rejected,
            VenueReportKind::Canceled => LegEvent::Canceled,
            VenueReportKind::Modified => LegEvent::Modified,
            VenueReportKind::ModifyRejected => LegEvent::ModifyRejected,
            VenueReportKind::CancelRejected => LegEvent::CancelRejected,
            VenueReportKind::Filled => LegEvent::Filled,
        }
    }
}

/// Why the fence refused a venue report. Every variant means the report is NOT
/// applied to the leg (fail-closed); the variant is for ops/logging and to let
/// the shell distinguish a benign late report from a genuine routing fault.
/// Evaluated client-id-first, then generation (see [`ExpectedIdentity::admit`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FenceReject {
    /// The report's client id differs from the leg's expected order — it was
    /// misrouted to the wrong order entirely.
    ForeignClientId,
    /// The report's client id matches but its generation is *older* than the
    /// leg's expected one — a late report for an order this leg already
    /// requoted away from. Applying it would tear down the successor order.
    StaleGeneration,
    /// The leg expects no order (nothing in flight) or the report's generation
    /// is one this leg has not reached — an order the maker never issued. The
    /// most conservative reject: trust nothing about an unknown order.
    UnknownOrder,
}

/// A per-leg holder of the order identity the leg currently expects venue
/// reports to be about, and the fence over incoming reports.
///
/// Constructed explicitly (no `Default`: the bolt-v3 legacy-default fence
/// forbids a `Default` impl on the production surface, and the leg must name its
/// starting state rather than inherit a zeroed one):
/// - [`ExpectedIdentity::idle`] — the leg has no order in flight; every report
///   is [`FenceReject::UnknownOrder`] until a submit establishes an identity.
/// - [`ExpectedIdentity::submitting`] — the leg has just emitted a submit for a
///   known [`OrderIdentity`]; reports for that identity are admitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedIdentity {
    /// The order the leg currently expects reports about, or `None` when nothing
    /// is in flight (a flat leg).
    expected: Option<OrderIdentity>,
}

impl ExpectedIdentity {
    /// A leg with nothing in flight — no expected order. Every report rejects as
    /// [`FenceReject::UnknownOrder`] until [`Self::submitting`] establishes an
    /// identity.
    pub fn idle() -> Self {
        Self { expected: None }
    }

    /// A leg that has just submitted `identity`; reports tagged with exactly that
    /// identity are admitted.
    pub fn submitting(identity: OrderIdentity) -> Self {
        Self {
            expected: Some(identity),
        }
    }

    /// The identity the leg currently expects, if any.
    pub fn expected(&self) -> Option<&OrderIdentity> {
        self.expected.as_ref()
    }

    /// Establish a fresh expected order — the new client id submitted on the
    /// next generation. The generation MUST advance past the prior one so the
    /// fresh order cannot inherit the stale order's in-flight reports; this is
    /// the requote bump the module's whole fail-closed guarantee rests on.
    ///
    /// Returns `false` and leaves the expected identity unchanged when
    /// `next.generation` does not strictly exceed the current expected
    /// generation — a non-advancing generation is a caller bug that would let a
    /// stale report pass as current, so it is refused (fail-closed).
    pub fn requote_to(&mut self, next: OrderIdentity) -> bool {
        if let Some(current) = &self.expected
            && next.generation <= current.generation
        {
            return false;
        }
        self.expected = Some(next);
        true
    }

    /// Clear the expected order — the leg is flat (its order fully filled,
    /// cancelled, or rejected). Subsequent reports reject as
    /// [`FenceReject::UnknownOrder`] until the next submit.
    pub fn clear(&mut self) {
        self.expected = None;
    }

    /// The fence: match a raw venue report to the leg's expected order and, only
    /// when the identity matches exactly, map its kind to the [`LegEvent`] the
    /// lifecycle machine may consume. Otherwise return a typed
    /// [`FenceReject`] and apply nothing (fail-closed).
    ///
    /// Match order, client-id first:
    /// - no expected order -> [`FenceReject::UnknownOrder`];
    /// - client id differs -> [`FenceReject::ForeignClientId`] (misrouted);
    /// - client id matches, generation older -> [`FenceReject::StaleGeneration`];
    /// - client id matches, generation newer -> [`FenceReject::UnknownOrder`]
    ///   (a generation the maker has not issued);
    /// - exact match -> `Ok(LegEvent)`.
    pub fn admit(&self, report: &VenueReport) -> Result<LegEvent, FenceReject> {
        let expected = self.expected.as_ref().ok_or(FenceReject::UnknownOrder)?;
        if report.client_order_id != expected.client_order_id {
            return Err(FenceReject::ForeignClientId);
        }
        if report.generation < expected.generation {
            return Err(FenceReject::StaleGeneration);
        }
        if report.generation > expected.generation {
            return Err(FenceReject::UnknownOrder);
        }
        Ok(report.kind.into_leg_event())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The set of venue report kinds and the lifecycle event each must map to —
    /// the full 1:1 mapping the fence emits on a clean identity match.
    const KIND_EVENT_PAIRS: [(VenueReportKind, LegEvent); 7] = [
        (VenueReportKind::Accepted, LegEvent::Accepted),
        (VenueReportKind::Rejected, LegEvent::Rejected),
        (VenueReportKind::Canceled, LegEvent::Canceled),
        (VenueReportKind::Modified, LegEvent::Modified),
        (VenueReportKind::ModifyRejected, LegEvent::ModifyRejected),
        (VenueReportKind::CancelRejected, LegEvent::CancelRejected),
        (VenueReportKind::Filled, LegEvent::Filled),
    ];

    fn cid(id: &str) -> ClientOrderId {
        ClientOrderId::new(id.to_string())
    }

    fn report(id: &str, generation: u64, kind: VenueReportKind) -> VenueReport {
        VenueReport {
            client_order_id: cid(id),
            generation,
            kind,
        }
    }

    #[test]
    fn current_identity_admits_every_kind_to_its_event() {
        // A leg expecting order "a" on generation 7 admits every venue report
        // kind tagged with that exact identity, mapped to its lifecycle event.
        let fence = ExpectedIdentity::submitting(OrderIdentity::new(cid("a"), 7));
        for (kind, expected_event) in KIND_EVENT_PAIRS {
            assert_eq!(
                fence.admit(&report("a", 7, kind)),
                Ok(expected_event),
                "{kind:?} on the current identity must map to its event"
            );
        }
    }

    #[test]
    fn an_older_generation_is_rejected_as_stale() {
        // Same client id, but a report for a generation the leg already requoted
        // away from: a late report for the predecessor order.
        let fence = ExpectedIdentity::submitting(OrderIdentity::new(cid("a"), 5));
        assert_eq!(
            fence.admit(&report("a", 4, VenueReportKind::Canceled)),
            Err(FenceReject::StaleGeneration),
            "a Canceled for the prior generation must not tear down the successor"
        );
        assert_eq!(
            fence.admit(&report("a", 0, VenueReportKind::Accepted)),
            Err(FenceReject::StaleGeneration)
        );
    }

    #[test]
    fn a_newer_generation_is_rejected_as_unknown() {
        // Same client id, but a generation the maker has not issued yet -> an
        // order it never submitted; trust nothing about it.
        let fence = ExpectedIdentity::submitting(OrderIdentity::new(cid("a"), 5));
        assert_eq!(
            fence.admit(&report("a", 6, VenueReportKind::Accepted)),
            Err(FenceReject::UnknownOrder)
        );
        assert_eq!(
            fence.admit(&report("a", u64::MAX, VenueReportKind::Filled)),
            Err(FenceReject::UnknownOrder)
        );
    }

    #[test]
    fn a_foreign_client_id_is_rejected_as_misrouted() {
        // A report for a different order entirely — even on the same generation.
        let fence = ExpectedIdentity::submitting(OrderIdentity::new(cid("a"), 5));
        assert_eq!(
            fence.admit(&report("b", 5, VenueReportKind::Accepted)),
            Err(FenceReject::ForeignClientId),
            "a matching generation must not rescue a foreign client id"
        );
        assert_eq!(
            fence.admit(&report("b", 4, VenueReportKind::Canceled)),
            Err(FenceReject::ForeignClientId),
            "client id is checked before generation"
        );
    }

    #[test]
    fn an_idle_leg_rejects_every_report_as_unknown() {
        // Nothing in flight: every report is about an order the leg does not
        // expect, regardless of kind or generation.
        let fence = ExpectedIdentity::idle();
        assert_eq!(fence.expected(), None);
        for (kind, _) in KIND_EVENT_PAIRS {
            assert_eq!(
                fence.admit(&report("a", 0, kind)),
                Err(FenceReject::UnknownOrder),
                "{kind:?} on an idle leg must reject as unknown"
            );
        }
    }

    #[test]
    fn requote_advances_the_generation_and_re_homes_the_fence() {
        // After a requote the new identity is admitted and the old one becomes
        // stale — the core anti-inheritance guarantee.
        let mut fence = ExpectedIdentity::submitting(OrderIdentity::new(cid("a"), 1));
        assert!(fence.requote_to(OrderIdentity::new(cid("b"), 2)));
        assert_eq!(fence.expected(), Some(&OrderIdentity::new(cid("b"), 2)));
        // The fresh order's reports are admitted.
        assert_eq!(
            fence.admit(&report("b", 2, VenueReportKind::Accepted)),
            Ok(LegEvent::Accepted)
        );
        // A late report for the order we requoted away from is now stale, not
        // applied to the successor.
        assert_eq!(
            fence.admit(&report("a", 1, VenueReportKind::Canceled)),
            Err(FenceReject::ForeignClientId),
            "the predecessor's client id is no longer the expected one"
        );
    }

    #[test]
    fn requote_to_a_non_advancing_generation_is_refused() {
        // A non-advancing generation would let a stale report pass as current;
        // refuse it and leave the expected identity untouched (fail-closed).
        let mut fence = ExpectedIdentity::submitting(OrderIdentity::new(cid("a"), 5));
        assert!(!fence.requote_to(OrderIdentity::new(cid("b"), 5)));
        assert!(!fence.requote_to(OrderIdentity::new(cid("b"), 4)));
        assert_eq!(
            fence.expected(),
            Some(&OrderIdentity::new(cid("a"), 5)),
            "a refused requote must not change the expected identity"
        );
    }

    #[test]
    fn requote_from_idle_establishes_the_first_identity() {
        // From idle there is no prior generation to advance past, so any
        // identity is accepted as the first expected order.
        let mut fence = ExpectedIdentity::idle();
        assert!(fence.requote_to(OrderIdentity::new(cid("a"), 0)));
        assert_eq!(
            fence.admit(&report("a", 0, VenueReportKind::Accepted)),
            Ok(LegEvent::Accepted)
        );
    }

    #[test]
    fn clear_returns_the_leg_to_unknown() {
        // After a terminal event the leg is flat; reports for the cleared order
        // reject as unknown until the next submit.
        let mut fence = ExpectedIdentity::submitting(OrderIdentity::new(cid("a"), 3));
        fence.clear();
        assert_eq!(fence.expected(), None);
        assert_eq!(
            fence.admit(&report("a", 3, VenueReportKind::Filled)),
            Err(FenceReject::UnknownOrder)
        );
    }

    #[test]
    fn accessors_round_trip_the_identity_parts() {
        let identity = OrderIdentity::new(cid("order-xyz"), 42);
        assert_eq!(identity.client_order_id().as_str(), "order-xyz");
        assert_eq!(identity.generation(), 42);
    }
}
