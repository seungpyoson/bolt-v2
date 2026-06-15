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
use crate::bolt_v3_requote_budget::{RequoteBudget, RequoteBudgetPair};

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
/// Fails closed (returns `Err`) on a malformed submit-rate string, an interval
/// that overflows the millisecond window, a zero REST cap, a zero
/// `min_interval_ms`, or a `min_interval_ms` that is not strictly below either
/// sliding window — any of which would throttle or refuse every reservation and
/// silently stop the maker quoting.
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

    if rest_cap_per_minute == 0 {
        return Err(
            "venue REST egress cap_per_minute must be > 0 (a zero REST budget refuses every quote)"
                .to_string(),
        );
    }
    if min_interval_ms == 0 {
        return Err(
            "requote_min_interval_ms must be > 0 (a zero anti-flicker floor disables the throttle)"
                .to_string(),
        );
    }
    if min_interval_ms >= submit_window_ms {
        return Err(format!(
            "requote_min_interval_ms ({min_interval_ms}) must be < the submit-rate window ({submit_window_ms} ms), otherwise every submit reservation is throttled and the maker never quotes"
        ));
    }
    if min_interval_ms >= MILLIS_PER_MINUTE_U64 {
        return Err(format!(
            "requote_min_interval_ms ({min_interval_ms}) must be < the REST window ({MILLIS_PER_MINUTE_U64} ms), otherwise every REST reservation is throttled and the maker never quotes"
        ));
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

    #[test]
    fn submit_cap_is_sourced_from_the_rate_string_limit() {
        // cap=2 from "2/00:01:00"; REST cap (100) is slack so the submit limit is
        // the binding constraint. Reservations are spaced past the anti-flicker
        // floor so only the window cap — not the min-interval — can refuse them.
        let mut pair = build_requote_budget_pair("2/00:01:00", 100, MIN_INTERVAL_MS)
            .expect("a well-formed rate string and non-zero caps build a budget pair");
        assert!(pair.try_reserve_fresh_submit(1_000), "first submit fits");
        assert!(
            pair.try_reserve_fresh_submit(2_000),
            "second submit fills the cap"
        );
        assert!(
            !pair.try_reserve_fresh_submit(3_000),
            "third submit must be refused by the submit cap of 2 parsed from the rate string"
        );
        assert_eq!(pair.submit_commands_in_window(), 2);
    }

    #[test]
    fn rest_cap_is_sourced_from_the_egress_cap_per_minute() {
        // REST cap=2 from the egress argument; submit cap (100) is slack so the
        // REST ceiling is the binding constraint. A fresh submit costs one REST
        // call, so the third is refused on the REST budget, not the submit budget.
        let mut pair = build_requote_budget_pair("100/00:01:00", 2, MIN_INTERVAL_MS)
            .expect("a well-formed rate string and non-zero caps build a budget pair");
        assert!(pair.try_reserve_fresh_submit(1_000), "first submit fits");
        assert!(
            pair.try_reserve_fresh_submit(2_000),
            "second submit fills the REST cap"
        );
        assert!(
            !pair.try_reserve_fresh_submit(3_000),
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
        let mut pair = build_requote_budget_pair("1/00:00:30", 10, MIN_INTERVAL_MS)
            .expect("a 30-second rate string builds a budget pair");
        assert!(pair.try_reserve_fresh_submit(1_000), "first submit fits");
        assert!(
            !pair.try_reserve_fresh_submit(29_000),
            "a submit 28s later is inside the 30s window and refused by the cap of 1"
        );
        assert!(
            pair.try_reserve_fresh_submit(32_000),
            "a submit 31s later is past the 30s window, so the first entry evicted and it fits"
        );
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
    fn a_zero_rest_cap_fails_closed() {
        let error = build_requote_budget_pair("40/00:01:00", 0, MIN_INTERVAL_MS)
            .expect_err("a zero REST egress cap must fail closed");
        assert!(error.contains("cap_per_minute must be > 0"), "{error}");
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
    fn a_min_interval_not_below_the_submit_window_fails_closed() {
        // "1/00:00:30" => 30_000 ms submit window; a 30_000 ms min-interval would
        // throttle every submit reservation.
        let error = build_requote_budget_pair("1/00:00:30", 100, 30_000)
            .expect_err("a min-interval at/above the submit window must fail closed");
        assert!(
            error.contains("must be < the submit-rate window"),
            "{error}"
        );
    }

    #[test]
    fn a_min_interval_not_below_the_rest_window_fails_closed() {
        // "1/02:00:00" => 7_200_000 ms submit window (passes the submit check) but
        // the REST window is the fixed 60_000 ms minute, so a 60_000 ms
        // min-interval must be rejected against the REST window.
        let error = build_requote_budget_pair("1/02:00:00", 100, 60_000)
            .expect_err("a min-interval at/above the REST window must fail closed");
        assert!(error.contains("must be < the REST window"), "{error}");
    }

    #[test]
    fn a_well_formed_configuration_builds_a_usable_pair() {
        // The canonical deploy shape: 40/min submit governor, 100/min CLOB REST
        // egress, 500 ms anti-flicker. Both budgets start empty and admit a fresh
        // co-quote.
        let mut pair = build_requote_budget_pair("40/00:01:00", 100, MIN_INTERVAL_MS)
            .expect("the canonical configuration builds a budget pair");
        assert_eq!(pair.submit_commands_in_window(), 0);
        assert_eq!(pair.rest_cost_in_window(), 0);
        assert!(
            pair.try_reserve_fresh_submit(1_000),
            "a fresh submit fits the canonical caps"
        );
        assert_eq!(pair.submit_commands_in_window(), 1);
        assert_eq!(pair.rest_cost_in_window(), 1);
    }
}
