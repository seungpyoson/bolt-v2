//! Pure, NautilusTrader-free venue-maintenance-window posture gate for the
//! binary-oracle maker (W6 — ops triggers: scheduled maintenance windows).
//!
//! Venues run periodic, *non-retryable* maintenance (e.g. a weekly platform
//! restart) during which order submission is rejected and any resting quotes are
//! at risk of being silently dropped or filled against a frozen book. The maker
//! must not wait to discover this by a stream of rejects — it must **pre-emptively
//! suspend new quoting and flatten** ahead of the window, stay flat through it,
//! and only resume once the venue is back. This module is the pure decision that
//! drives that posture; the NT strategy shell owns the actual
//! suspend/cancel/flatten and sources the window schedule from config.
//!
//! ## Where this sits in the governor vocabulary (NO DUAL PATHS)
//!
//! [`crate::strategies::maker_governor::MakerGovernorState::CancelOnly`] is
//! documented as the posture for a "maintenance window / reconnect-in-progress"
//! but the W3 market-and-inventory governor *never produces it*. This module is
//! the W6 producer of that signal: [`maintenance_governor_state`] maps a
//! [`MaintenancePosture`] onto exactly that existing `CancelOnly` variant rather
//! than inventing a parallel ops enum. The downstream consumer is already wired —
//! [`crate::strategies::maker_resync::cancel_all_on_kill`] treats `CancelOnly` as
//! a terminal pull that drains both legs ("ops pull: maintenance / reconnect").
//! So the chain is: schedule (config) → this module → `CancelOnly` → drain.
//!
//! ## The schedule is an INPUT, not venue-contract state
//!
//! [`crate::venue_contract::VenueContract`] carries no maintenance field — a
//! maintenance schedule is operational, changes independently of the adapter
//! capability surface, and is sourced by the shell from TOML config. This module
//! therefore takes the window as explicit parameters (`now`, `window_start`,
//! `window_duration`, `pre_flatten_lead`) and owns none of it.
//!
//! ## Units (single convention, no clock)
//!
//! Every instant and span is **integer milliseconds since an arbitrary but
//! consistent epoch**, as a `u64` — the same convention as the sibling
//! [`crate::strategies::requote_budget`] throttle (`now_ms`, `window_ms`). The
//! caller passes the current time in; this module never reads a clock and holds
//! no NT type, so it is exhaustively unit-testable. Integer milliseconds also
//! mean there are no floats on the decision path and therefore no NaN/Inf class
//! of bug to guard — the only degenerate inputs are arithmetic ones (a zero-length
//! window, or an end that overflows), handled below.
//!
//! ## Fail-closed direction: toward SUSPEND
//!
//! A maintenance gate exists to *stop* quoting around a risky window, so every
//! degenerate input fails **toward suspension**, never toward "keep quoting":
//!
//! - A `window_duration` of zero is not a real window. Rather than treat it as
//!   "no maintenance" (fail open — keep quoting), [`maintenance_posture`] returns
//!   `None`, which the shell treats as "do not quote", and
//!   [`maintenance_governor_state`] maps that `None` to `CancelOnly`. A
//!   misconfigured/empty window thus suspends rather than silently disabling the
//!   guard.
//! - A `window_start + window_duration` that would overflow `u64` is likewise
//!   treated as degenerate and fails closed to `None` (detected precisely with
//!   `checked_add`; a window ending exactly on `u64::MAX` is still a valid window,
//!   so only a true overflow is rejected, never that boundary). The lead-up start
//!   uses a saturating subtraction and so cannot overflow — an over-large
//!   `pre_flatten_lead` just opens the lead-up at 0.
//!
//! There is deliberately no `Default`: the bolt-v3 legacy-default fence forbids
//! it, and an ops gate must name the schedule it was handed rather than inherit a
//! zero window that would fail closed on every tick.

use crate::strategies::maker_governor::MakerGovernorState;

/// The maker's posture relative to a single venue maintenance window.
///
/// A graduated three-state gate (not a boolean) so the pre-window flatten is
/// distinct from being inside the window — the shell may, for example, log them
/// differently or only begin reducing during `PreFlatten`. Both non-`Clear`
/// postures map to the same suspend-and-pull governor state; the distinction is
/// for ops/observability and for the lead-up flatten.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenancePosture {
    /// Outside the window and outside its lead-up: normal quoting is permitted.
    /// (Whether the maker *actually* quotes is still the W3 governor's call; this
    /// posture only declines to veto.)
    Clear,
    /// Inside the pre-flatten lead-up immediately before the window: suspend new
    /// quotes and begin flattening so the maker is flat by the time the window
    /// opens.
    PreFlatten,
    /// Inside the maintenance window itself: stay suspended and flat — order
    /// submission is expected to be rejected or unsafe.
    InWindow,
}

/// Resolve the maintenance posture at `now` for a window that opens at
/// `window_start`, lasts `window_duration`, with a `pre_flatten_lead` suspend-and-
/// flatten lead-up before it. All four arguments are **integer milliseconds** on
/// the same consistent epoch (see the module docs).
///
/// Timeline (all bounds milliseconds):
///
/// ```text
///        Clear            PreFlatten          InWindow            Clear
/// ───────────────────|──────────────────|──────────────────|───────────────────▶
///                 lead_start         window_start        window_end          now
///   now < lead_start    lead_start ≤ now    window_start ≤ now    now ≥ window_end
///                          < window_start      < window_end
/// ```
///
/// where `lead_start = window_start − pre_flatten_lead` and
/// `window_end = window_start + window_duration`.
///
/// Boundaries are half-open `[start, end)` so each instant has exactly one
/// posture: the `pre_flatten_lead` is `[lead_start, window_start)`, the window is
/// `[window_start, window_end)`, and the maker is `Clear` again at exactly
/// `window_end`.
///
/// Returns:
/// - `Some(MaintenancePosture::Clear)` well before the lead-up or at/after the
///   window end;
/// - `Some(MaintenancePosture::PreFlatten)` inside the lead-up;
/// - `Some(MaintenancePosture::InWindow)` inside the window;
/// - `None` for a **degenerate window** — a `window_duration` of zero, or a
///   `window_start + window_duration` that overflows `u64`. `None` is the
///   fail-closed sentinel meaning "treat as do not quote"; the shell suspends
///   rather than trusting a malformed schedule. Use
///   [`maintenance_governor_state`] to fold `None` into the governor vocabulary.
///
/// `pre_flatten_lead` may legitimately be zero — a venue may give no warning, so
/// the maker only suspends once the window opens; that is not degenerate, only the
/// lead-up collapses to empty.
pub fn maintenance_posture(
    now: u64,
    window_start: u64,
    window_duration: u64,
    pre_flatten_lead: u64,
) -> Option<MaintenancePosture> {
    // A zero-length window is not a real window. Fail closed: a misconfigured or
    // empty schedule must suspend, not silently disable the gate.
    if window_duration == 0 {
        return None;
    }

    // Detect a window end that overflows u64 precisely via checked_add and fail
    // closed on it — a corrupt schedule, never a window that "never ends". A
    // window whose end lands exactly on u64::MAX is a *valid* (if astronomical)
    // window, so only a true overflow (None) is rejected, not that boundary value.
    // `?` propagates that overflow None as the fail-closed "do not quote" sentinel.
    let window_end = window_start.checked_add(window_duration)?;
    // The lead-up start saturates downward, so it can never overflow: a
    // pre_flatten_lead larger than window_start simply opens the lead-up at 0.
    let lead_start = window_start.saturating_sub(pre_flatten_lead);

    if now >= window_end {
        // After the window: back to normal.
        Some(MaintenancePosture::Clear)
    } else if now >= window_start {
        // Inside the window.
        Some(MaintenancePosture::InWindow)
    } else if now >= lead_start {
        // Inside the pre-flatten lead-up.
        Some(MaintenancePosture::PreFlatten)
    } else {
        // Well before the lead-up.
        Some(MaintenancePosture::Clear)
    }
}

/// Fold a maintenance posture (including the `None` fail-closed sentinel from
/// [`maintenance_posture`]) onto the shared
/// [`crate::strategies::maker_governor::MakerGovernorState`] vocabulary — the W6
/// producer of the `CancelOnly` signal the W3 governor documents but never emits.
///
/// Mapping:
///
/// | posture                    | governor state | why                          |
/// |----------------------------|----------------|------------------------------|
/// | `Some(InWindow)`           | `CancelOnly`   | suspend + pull through window|
/// | `Some(PreFlatten)`         | `CancelOnly`   | suspend + flatten ahead      |
/// | `Some(Clear)`              | `Quoting`      | permit quoting (no veto)     |
/// | `None` (degenerate window) | `CancelOnly`   | fail closed → suspend + pull |
///
/// Both suspend postures and the degenerate-window sentinel map to `CancelOnly`,
/// which [`crate::strategies::maker_resync::cancel_all_on_kill`] then turns into a
/// both-legs drain. `Clear` resolves to `Quoting`, which only declares "this gate
/// does not veto" — the W3 market/inventory governor still has the final say on
/// the tick (safety always wins; this gate never *upgrades* a kill back to
/// quoting, it is consulted alongside the governor by the shell).
pub fn maintenance_governor_state(posture: Option<MaintenancePosture>) -> MakerGovernorState {
    match posture {
        Some(MaintenancePosture::Clear) => MakerGovernorState::Quoting,
        Some(MaintenancePosture::PreFlatten) | Some(MaintenancePosture::InWindow) | None => {
            MakerGovernorState::CancelOnly
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bolt_v3_numeric::MILLIS_PER_SECOND_U64;

    // Test-only window fixture, expressed in seconds for readability and scaled to
    // the module's millisecond convention. Literals here are test-only (the
    // source-fence strips #[cfg(test)]).
    const WINDOW_START_S: u64 = 1_000;
    const WINDOW_DURATION_S: u64 = 600; // a ten-minute window
    const PRE_FLATTEN_LEAD_S: u64 = 120; // suspend two minutes ahead

    fn ms(seconds: u64) -> u64 {
        seconds * MILLIS_PER_SECOND_U64
    }

    /// The standard window used across the timeline tests.
    fn window() -> (u64, u64, u64) {
        (
            ms(WINDOW_START_S),
            ms(WINDOW_DURATION_S),
            ms(PRE_FLATTEN_LEAD_S),
        )
    }

    #[test]
    fn clear_well_before_the_lead_up() {
        let (start, duration, lead) = window();
        // Long before the lead-up begins.
        let now = ms(WINDOW_START_S - PRE_FLATTEN_LEAD_S - 60);
        assert_eq!(
            maintenance_posture(now, start, duration, lead),
            Some(MaintenancePosture::Clear)
        );
    }

    #[test]
    fn pre_flatten_at_the_lead_boundary_and_within() {
        let (start, duration, lead) = window();
        let lead_start = ms(WINDOW_START_S - PRE_FLATTEN_LEAD_S);
        // Exactly at lead_start the half-open [lead_start, window_start) opens.
        assert_eq!(
            maintenance_posture(lead_start, start, duration, lead),
            Some(MaintenancePosture::PreFlatten),
            "lead-up is inclusive at its start"
        );
        // One millisecond inside the lead-up.
        assert_eq!(
            maintenance_posture(lead_start + 1, start, duration, lead),
            Some(MaintenancePosture::PreFlatten)
        );
        // One millisecond before lead_start is still Clear.
        assert_eq!(
            maintenance_posture(lead_start - 1, start, duration, lead),
            Some(MaintenancePosture::Clear),
            "the instant before the lead-up is still Clear"
        );
    }

    #[test]
    fn in_window_at_the_open_and_within() {
        let (start, duration, lead) = window();
        // Exactly at window_start the window opens (and the lead-up closes).
        assert_eq!(
            maintenance_posture(start, start, duration, lead),
            Some(MaintenancePosture::InWindow),
            "window is inclusive at its start"
        );
        // Mid-window.
        assert_eq!(
            maintenance_posture(start + ms(WINDOW_DURATION_S / 2), start, duration, lead),
            Some(MaintenancePosture::InWindow)
        );
        // One millisecond before the end is still in-window.
        let window_end = start + duration;
        assert_eq!(
            maintenance_posture(window_end - 1, start, duration, lead),
            Some(MaintenancePosture::InWindow)
        );
    }

    #[test]
    fn clear_after_at_the_window_end_and_beyond() {
        let (start, duration, lead) = window();
        let window_end = start + duration;
        // Exactly at window_end the half-open window has closed → Clear.
        assert_eq!(
            maintenance_posture(window_end, start, duration, lead),
            Some(MaintenancePosture::Clear),
            "window is exclusive at its end — Clear at exactly window_end"
        );
        // Well after the window.
        assert_eq!(
            maintenance_posture(window_end + ms(60), start, duration, lead),
            Some(MaintenancePosture::Clear)
        );
    }

    #[test]
    fn zero_lead_collapses_the_lead_up_but_is_not_degenerate() {
        let start = ms(WINDOW_START_S);
        let duration = ms(WINDOW_DURATION_S);
        // No warning: the lead-up is empty, so the instant before the window is
        // Clear and the window open is the first suspend.
        assert_eq!(
            maintenance_posture(start - 1, start, duration, 0),
            Some(MaintenancePosture::Clear),
            "with zero lead there is no PreFlatten region"
        );
        assert_eq!(
            maintenance_posture(start, start, duration, 0),
            Some(MaintenancePosture::InWindow)
        );
    }

    #[test]
    fn zero_duration_window_fails_closed_to_none() {
        // A zero-length window is degenerate: fail closed (suspend), do not treat
        // it as "no maintenance / keep quoting".
        let start = ms(WINDOW_START_S);
        let lead = ms(PRE_FLATTEN_LEAD_S);
        assert_eq!(
            maintenance_posture(start, start, 0, lead),
            None,
            "zero-duration window must fail closed to None"
        );
        // It fails closed regardless of where `now` falls.
        assert_eq!(maintenance_posture(0, start, 0, lead), None);
        assert_eq!(maintenance_posture(start + ms(60), start, 0, lead), None);
    }

    #[test]
    fn end_overflow_fails_closed_to_none() {
        // A start + duration that overflows u64 is a corrupt schedule → fail
        // closed, never a window that "never ends".
        assert_eq!(
            maintenance_posture(0, u64::MAX, 1, 0),
            None,
            "window_end overflow must fail closed to None"
        );
        assert_eq!(maintenance_posture(0, u64::MAX - 1, 10, 0), None);
    }

    #[test]
    fn a_window_ending_exactly_at_u64_max_is_valid_not_degenerate() {
        // window_end == u64::MAX is a real (astronomical) window, NOT an overflow:
        // checked_add returns Some, so the maker is genuinely InWindow rather than
        // being rejected to None by a sentinel-equality false positive.
        let window_start = u64::MAX - 100;
        let window_duration = 100; // end == u64::MAX exactly, no overflow
        assert_eq!(
            maintenance_posture(u64::MAX - 50, window_start, window_duration, 0),
            Some(MaintenancePosture::InWindow),
            "a window ending exactly at u64::MAX is valid and InWindow"
        );
    }

    #[test]
    fn a_lead_larger_than_window_start_opens_the_lead_up_at_zero() {
        // pre_flatten_lead larger than window_start cannot overflow — lead_start
        // saturates to 0, so the lead-up is simply [0, window_start), a valid
        // PreFlatten region, not the degenerate None the deleted guard produced.
        let start = ms(WINDOW_START_S);
        let duration = ms(WINDOW_DURATION_S);
        let huge_lead = start + ms(60); // larger than window_start
        assert_eq!(
            maintenance_posture(0, start, duration, huge_lead),
            Some(MaintenancePosture::PreFlatten),
            "an over-large lead opens the lead-up at 0, not None"
        );
        assert_eq!(
            maintenance_posture(start - 1, start, duration, huge_lead),
            Some(MaintenancePosture::PreFlatten)
        );
    }

    #[test]
    fn suspend_postures_map_to_cancel_only() {
        // Both PreFlatten and InWindow are suspend-and-pull → CancelOnly, the W6
        // signal the W3 governor documents but never emits.
        assert_eq!(
            maintenance_governor_state(Some(MaintenancePosture::PreFlatten)),
            MakerGovernorState::CancelOnly
        );
        assert_eq!(
            maintenance_governor_state(Some(MaintenancePosture::InWindow)),
            MakerGovernorState::CancelOnly
        );
    }

    #[test]
    fn clear_maps_to_quoting() {
        // Clear declines to veto — the W3 governor still decides the tick.
        assert_eq!(
            maintenance_governor_state(Some(MaintenancePosture::Clear)),
            MakerGovernorState::Quoting
        );
    }

    #[test]
    fn degenerate_none_maps_to_cancel_only() {
        // The fail-closed sentinel from a degenerate window suspends.
        assert_eq!(
            maintenance_governor_state(None),
            MakerGovernorState::CancelOnly
        );
    }

    #[test]
    fn full_timeline_walks_clear_pre_flatten_in_window_clear() {
        // One window, sampled across the whole timeline, must visit each posture
        // exactly in order with no gaps.
        let (start, duration, lead) = window();
        let lead_start = ms(WINDOW_START_S - PRE_FLATTEN_LEAD_S);
        let window_end = start + duration;
        let samples = [
            (lead_start - 1, MaintenancePosture::Clear),
            (lead_start, MaintenancePosture::PreFlatten),
            (start - 1, MaintenancePosture::PreFlatten),
            (start, MaintenancePosture::InWindow),
            (window_end - 1, MaintenancePosture::InWindow),
            (window_end, MaintenancePosture::Clear),
        ];
        for (now, expected) in samples {
            assert_eq!(
                maintenance_posture(now, start, duration, lead),
                Some(expected),
                "at now={now} posture must be {expected:?}"
            );
        }
    }
}
