//! Shared maker requote-budget construction from rate config + venue egress.
//!
//! The maker's two-budget requote governor (submit commands + venue REST calls)
//! must be initialized from the same operator/venue sources the rest of bolt-v3
//! reconciles order rates against — never from literals. This bridge parses the
//! NT submit-rate string and the venue REST egress cap into a
//! [`RequoteBudgetPair`] so the strategy shell constructs the governor with zero
//! hardcoded caps or windows, and fails closed on any degenerate input.
//!
//! Sourcing mirrors `bolt_v3_validate::validate_order_rate_within_venue_egress`:
//! the submit governor comes from `risk.nautilus.max_order_submit_rate` (parsed
//! by the single shared [`crate::bolt_v3_validate::validate_rate_limit_string`])
//! and the REST ceiling from the execution venue's
//! `VenueEgressModel::cap_per_minute`.

use crate::bolt_v3_numeric::{MILLIS_PER_MINUTE_U64, MILLIS_PER_SECOND_U64};
use crate::bolt_v3_requote_budget::{CANCEL_RESUBMIT_REST_COST, RequoteBudget, RequoteBudgetPair};

/// Build the maker's two-budget requote governor from configured rates.
///
/// - `submit_rate` is the NT `limit/HH:MM:SS` submit-governor string from
///   `risk.nautilus.max_order_submit_rate`; its `(limit, interval_seconds)` is
///   parsed by the single shared
///   [`crate::bolt_v3_validate::validate_rate_limit_string`] so there is exactly
///   one rate-string interpretation in the binary.
/// - `rest_cap_per_minute` is the execution venue's modeled REST egress ceiling
///   (`VenueEgressModel::cap_per_minute`), the same source the config validator
///   reconciles order rates against.
/// - `min_interval_ms` is the operator anti-flicker floor applied to BOTH
///   sub-budgets.
///
/// The submit-command floor is delegated to
/// [`crate::bolt_v3_validate::validate_rate_limit_string`], which guarantees the
/// parsed `limit` is `>= 1` (one submit command), so the submit budget can always
/// admit at least one submit; this builder does not re-assert that invariant
/// (single source of truth for the submit-rate floor).
///
/// Fails closed (returns `Err`) on: a malformed submit-rate string; an interval
/// that overflows the millisecond window; a REST cap below the cost of a single
/// cancel+resubmit reprice (which would build a budget that can place a quote but
/// never move it); or a zero `min_interval_ms` (which disables the anti-flicker
/// floor). A `min_interval_ms` at or above a sliding window is NOT rejected — it
/// is a valid, conservative cadence: the first reservation is granted and later
/// reservations are granted once the interval elapses, so the maker still quotes.
pub fn build_requote_budget_pair(
    submit_rate: &str,
    rest_cap_per_minute: u32,
    min_interval_ms: u64,
) -> Result<RequoteBudgetPair, String> {
    let (submit_limit, submit_interval_seconds) =
        crate::bolt_v3_validate::validate_rate_limit_string(submit_rate).map_err(|error| {
            format!("max_order_submit_rate `{submit_rate}` is not a valid rate string: {error}")
        })?;
    let submit_window_ms = submit_interval_seconds
        .checked_mul(MILLIS_PER_SECOND_U64)
        .ok_or_else(|| {
            format!(
                "max_order_submit_rate `{submit_rate}` interval ({submit_interval_seconds}s) is invalid: it overflows u64 milliseconds"
            )
        })?;

    // The REST budget must admit the most expensive single reservation the maker
    // can issue: a cancel+resubmit reprice, which costs `CANCEL_RESUBMIT_REST_COST`
    // REST calls (the venue has no in-place modify). A cap below that floor builds a
    // budget that grants a fresh quote but can NEVER reprice it, silently stranding
    // the maker. The floor is sourced from the same structural reprice-cost constant
    // the budget charges, so there is one source of truth for the reprice REST cost.
    if u64::from(rest_cap_per_minute) < CANCEL_RESUBMIT_REST_COST {
        return Err(format!(
            "venue REST egress cap_per_minute ({rest_cap_per_minute}) must be >= the cancel+resubmit reprice cost ({CANCEL_RESUBMIT_REST_COST} REST calls); a smaller REST budget can place a quote but can never reprice it, silently stranding the maker"
        ));
    }
    if min_interval_ms == 0 {
        return Err(
            "requote_min_interval_ms must be > 0 (a zero anti-flicker floor disables the throttle)"
                .to_string(),
        );
    }

    Ok(RequoteBudgetPair::new(
        RequoteBudget::new(submit_limit, submit_window_ms, min_interval_ms),
        RequoteBudget::new(
            u64::from(rest_cap_per_minute),
            MILLIS_PER_MINUTE_U64,
            min_interval_ms,
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN_INTERVAL_MS: u64 = 500;

    fn reserve_proposal(
        pair: &RequoteBudgetPair,
        proposal: Result<
            crate::bolt_v3_requote_budget::RequoteBudgetReservationProposal,
            crate::bolt_v3_requote_budget::RequoteBudgetReservationDenied,
        >,
    ) -> bool {
        proposal
            .and_then(|proposal| pair.reserve(proposal))
            .and_then(crate::bolt_v3_requote_budget::RequoteBudgetReservation::commit)
            .is_ok()
    }

    fn reserve_fresh(pair: &RequoteBudgetPair, now_ms: u64) -> bool {
        reserve_proposal(pair, pair.propose_fresh_submit(now_ms))
    }

    fn reserve_cancel_resubmit(pair: &RequoteBudgetPair, now_ms: u64) -> bool {
        reserve_proposal(pair, pair.propose_cancel_resubmit(now_ms))
    }

    fn reserve_rest(pair: &RequoteBudgetPair, now_ms: u64) -> bool {
        reserve_proposal(pair, pair.propose_rest(now_ms))
    }

    #[test]
    fn submit_cap_is_sourced_from_the_rate_string_limit() {
        // cap=2 from "2/00:01:00"; REST cap (100) is slack so the submit limit is
        // the binding constraint. Reservations are spaced past the anti-flicker
        // floor so only the window cap — not the min-interval — can refuse them.
        let pair = build_requote_budget_pair("2/00:01:00", 100, MIN_INTERVAL_MS)
            .expect("a well-formed rate string and non-zero caps build a budget pair");
        assert!(reserve_fresh(&pair, 1_000), "first submit fits");
        assert!(reserve_fresh(&pair, 2_000), "second submit fills the cap");
        assert!(
            !reserve_fresh(&pair, 3_000),
            "third submit must be refused by the submit cap of 2 parsed from the rate string"
        );
        assert_eq!(pair.submit_commands_in_window(), 2);
    }

    #[test]
    fn rest_cap_is_sourced_from_the_egress_cap_per_minute() {
        // REST cap=2 from the egress argument; submit cap (100) is slack so the
        // REST ceiling is the binding constraint. A fresh submit costs one REST
        // call, so the third is refused on the REST budget, not the submit budget.
        let pair = build_requote_budget_pair("100/00:01:00", 2, MIN_INTERVAL_MS)
            .expect("a well-formed rate string and non-zero caps build a budget pair");
        assert!(reserve_fresh(&pair, 1_000), "first submit fits");
        assert!(
            reserve_fresh(&pair, 2_000),
            "second submit fills the REST cap"
        );
        assert!(
            !reserve_fresh(&pair, 3_000),
            "third submit must be refused by the REST egress cap of 2"
        );
        assert_eq!(pair.rest_cost_in_window(), 2);
    }

    #[test]
    fn submit_window_is_the_parsed_interval_not_a_hardcoded_minute() {
        // "1/00:00:30" => cap 1, window 30s. A reservation at +28s is inside the
        // 30s window and must be refused (cap 1 still held). A reservation at +31s
        // is past the 30s window so the first reservation has evicted and it must
        // succeed. If the window were hardcoded to 60_000 ms, the +31s reservation
        // would still see the first entry and fail — so this fails on a hardcoded
        // minute and passes only when the window tracks the parsed 30s interval.
        let pair = build_requote_budget_pair("1/00:00:30", 10, MIN_INTERVAL_MS)
            .expect("a 30-second rate string builds a budget pair");
        assert!(reserve_fresh(&pair, 1_000), "first submit fits");
        assert!(
            !reserve_fresh(&pair, 29_000),
            "a submit 28s later is inside the 30s window and refused by the cap of 1"
        );
        assert!(
            reserve_fresh(&pair, 32_000),
            "a submit 31s later is past the 30s window, so the first entry evicted and it fits"
        );
    }

    #[test]
    fn the_two_budgets_are_not_swapped_at_construction() {
        // A tandem swap of the two RequoteBudget args is invisible to the symmetric
        // fresh-submit (1 submit / 1 REST) sourcing tests above, so it needs an
        // ASYMMETRIC charge. "1/00:01:00" gives a submit cap of 1; the REST cap is a
        // slack 100. A single cancel+resubmit costs 1 submit + 2 REST: correctly
        // placed, the cap-1 submit budget admits the 1 command and the cap-100 REST
        // budget admits the 2 calls, so it is granted. If the two budgets were
        // swapped, the 2-REST charge would hit the cap-1 budget and be refused — so
        // this grant FAILS on a swap, which a symmetric reservation could not detect.
        let pair = build_requote_budget_pair("1/00:01:00", 100, MIN_INTERVAL_MS)
            .expect("a well-formed config builds a pair");
        assert!(
            reserve_cancel_resubmit(&pair, 1_000),
            "a single reprice (1 submit + 2 REST) is granted; a swapped pair refuses the 2-REST charge on the cap-1 submit budget"
        );
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 2);
    }

    #[test]
    fn a_malformed_submit_rate_fails_closed() {
        let error = build_requote_budget_pair("not-a-rate", 100, MIN_INTERVAL_MS)
            .expect_err("a malformed submit-rate string must fail closed");
        assert!(error.contains("not a valid rate string"), "{error}");
    }

    #[test]
    fn an_interval_that_overflows_millis_fails_closed() {
        // The parser accepts this (hours * 3600 stays under u64::MAX) but the
        // second-to-millisecond conversion overflows u64, so the bridge must fail
        // closed rather than saturate to a bogus window.
        let error = build_requote_budget_pair("1/5124095576031:00:00", 100, MIN_INTERVAL_MS)
            .expect_err("an interval whose millisecond conversion overflows must fail closed");
        assert!(error.contains("overflows u64 milliseconds"), "{error}");
    }

    #[test]
    fn a_rest_cap_below_the_reprice_cost_fails_closed() {
        // The decisive differential against the old `== 0`-only guard: a REST cap of
        // 1 is non-zero so the old guard ACCEPTED it, yet a cancel+resubmit reprice
        // costs 2 REST calls, so the built budget could place a quote but never move
        // it. The floor is the structural reprice cost, so 1 (and 0) must fail closed.
        let one = build_requote_budget_pair("40/00:01:00", 1, MIN_INTERVAL_MS)
            .expect_err("a REST cap below the reprice cost must fail closed");
        assert!(one.contains("can never reprice"), "{one}");
        let zero = build_requote_budget_pair("40/00:01:00", 0, MIN_INTERVAL_MS)
            .expect_err("a zero REST egress cap must fail closed");
        assert!(zero.contains("can never reprice"), "{zero}");
    }

    #[test]
    fn a_zero_min_interval_fails_closed() {
        let error = build_requote_budget_pair("40/00:01:00", 100, 0)
            .expect_err("a zero anti-flicker floor must fail closed");
        assert!(
            error.contains("requote_min_interval_ms must be > 0"),
            "{error}"
        );
    }

    #[test]
    fn a_min_interval_at_the_submit_window_still_grants_over_time() {
        // A min-interval EQUAL to the submit window is a valid conservative cadence,
        // not a degenerate config: the budget is not permanently closed. This pins
        // the removal of the old `min_interval >= submit_window` fail-closed guard by
        // reproducing the behavior it wrongly rejected — the first quote is granted,
        // a quote inside the interval is throttled, and a quote past the window is
        // granted again. "1/00:00:30" => 30_000 ms submit window; min-interval 30_000.
        let pair = build_requote_budget_pair("1/00:00:30", 100, 30_000)
            .expect("a min-interval equal to the submit window is a valid cadence");
        assert!(reserve_fresh(&pair, 1_000), "first quote is granted");
        assert!(
            !reserve_fresh(&pair, 2_000),
            "a quote 1s later is throttled by the 30s anti-flicker floor"
        );
        assert!(
            reserve_fresh(&pair, 31_001),
            "a quote past the 30s window/floor is granted again — not permanently closed"
        );
    }

    #[test]
    fn a_min_interval_at_the_rest_window_still_grants_over_time() {
        // The mirror for the REST window: a min-interval equal to the fixed 60_000 ms
        // REST window is also valid, pinning removal of the old `min_interval >=
        // MILLIS_PER_MINUTE_U64` guard. A 10/2h submit rate keeps the submit cap slack
        // so the REST-window min-interval is the only thing under test.
        let pair = build_requote_budget_pair("10/02:00:00", 100, 60_000)
            .expect("a min-interval equal to the REST window is a valid cadence");
        assert!(reserve_fresh(&pair, 1_000), "first quote is granted");
        assert!(
            !reserve_fresh(&pair, 2_000),
            "a quote 1s later is throttled by the 60s anti-flicker floor"
        );
        assert!(
            reserve_fresh(&pair, 61_001),
            "a quote past the 60s REST window/floor is granted again — not permanently closed"
        );
    }

    #[test]
    fn a_min_interval_above_the_submit_window_still_grants_over_time() {
        // The decisive case for the removed `min_interval >= submit_window` guard: a
        // min-interval STRICTLY GREATER than the window. The equality test above is
        // survived by a guard re-added as `>` (30_000 > 30_000 is false, so a `>`
        // guard would ACCEPT the equality config and that test would not flip). This
        // case flips under BOTH a `>=` and a `>` re-addition, pinning the whole
        // removal. "1/00:00:30" => 30_000 ms submit window; min-interval 45_000 (> it).
        let pair = build_requote_budget_pair("1/00:00:30", 100, 45_000)
            .expect("a min-interval above the submit window is a valid cadence");
        assert!(reserve_fresh(&pair, 1_000), "first quote is granted");
        assert!(
            !reserve_fresh(&pair, 2_000),
            "a quote 1s later is throttled by the 45s anti-flicker floor"
        );
        assert!(
            reserve_fresh(&pair, 46_001),
            "a quote past the 45s floor is granted again — the submit window evicted the first emit"
        );
    }

    #[test]
    fn a_min_interval_above_the_rest_window_still_grants_over_time() {
        // The mirror beyond the fixed 60_000 ms REST window, pinning removal of the
        // `min_interval >= MILLIS_PER_MINUTE_U64` guard against both `>=` and `>`
        // re-additions. A 10/2h submit rate keeps the submit cap and window slack so
        // the 90_000 ms min-interval (> the 60s REST window) is the only thing tested.
        let pair = build_requote_budget_pair("10/02:00:00", 100, 90_000)
            .expect("a min-interval above the REST window is a valid cadence");
        assert!(reserve_fresh(&pair, 1_000), "first quote is granted");
        assert!(
            !reserve_fresh(&pair, 2_000),
            "a quote 1s later is throttled by the 90s anti-flicker floor"
        );
        assert!(
            reserve_fresh(&pair, 91_001),
            "a quote past the 90s floor is granted again — the REST window evicted the first emit"
        );
    }

    #[test]
    fn the_min_interval_floor_is_plumbed_into_both_sub_budgets() {
        // The success tests above space reservations PAST the floor, so none proves
        // the configured `min_interval_ms` actually reached the RequoteBudget::new
        // calls. A standalone cancel charges only the REST budget, advancing only its
        // min-interval floor: a fresh submit a short tick later is then refused on the
        // REST floor (the submit budget's floor is still pristine — last_emit None),
        // proving the REST budget received the 500ms floor. If the builder had passed
        // 0 to the REST budget, the cancel would set no active floor and the submit
        // would be granted. (There is no submit-only reservation through the pair API,
        // so the submit budget's floor cannot be isolated the same way; the builder
        // passes the one `min_interval_ms` value to BOTH RequoteBudget::new calls.)
        let rest_floor = build_requote_budget_pair("40/00:01:00", 100, MIN_INTERVAL_MS)
            .expect("a well-formed config builds a pair");
        assert!(
            reserve_rest(&rest_floor, 1_000),
            "a standalone cancel is granted"
        );
        assert!(
            !reserve_fresh(&rest_floor, 1_100),
            "a submit 100ms after the cancel is refused on the REST min-interval floor"
        );
        assert!(
            reserve_fresh(&rest_floor, 1_500),
            "once the 500ms REST floor clears the submit is granted"
        );

        // And the floor is live (and is exactly the configured value, not 0) on the
        // fresh-submit path: two distinct-tick submits inside 500ms are throttled, and
        // a submit at the 500ms boundary is admitted.
        let submit_floor = build_requote_budget_pair("40/00:01:00", 100, MIN_INTERVAL_MS)
            .expect("a well-formed config builds a pair");
        assert!(
            reserve_fresh(&submit_floor, 1_000),
            "first submit is granted"
        );
        assert!(
            !reserve_fresh(&submit_floor, 1_100),
            "a second submit 100ms later is throttled by the 500ms floor"
        );
        assert!(
            reserve_fresh(&submit_floor, 1_500),
            "a submit at the 500ms boundary is admitted"
        );
    }

    #[test]
    fn a_well_formed_configuration_builds_a_usable_pair() {
        // The canonical deploy shape: 40/min submit governor, 100/min CLOB REST
        // egress, 500 ms anti-flicker. Both budgets start empty and admit a fresh
        // co-quote.
        let pair = build_requote_budget_pair("40/00:01:00", 100, MIN_INTERVAL_MS)
            .expect("the canonical configuration builds a budget pair");
        assert_eq!(pair.submit_commands_in_window(), 0);
        assert_eq!(pair.rest_cost_in_window(), 0);
        assert!(
            reserve_fresh(&pair, 1_000),
            "a fresh submit fits the canonical caps"
        );
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 1);
    }
}
