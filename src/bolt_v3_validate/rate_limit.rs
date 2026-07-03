use super::*;

/// Seconds in one hour / one minute, named so the `HH:MM:SS` interval
/// computation reads as a time conversion rather than bare magic numbers.
const SECONDS_PER_HOUR: u64 = 3600;
const SECONDS_PER_MINUTE: u64 = 60;

/// Validates an NT `limit/HH:MM:SS` rate-limit string and returns the parsed
/// `(limit, interval_seconds)` so callers can reconcile the rate against a
/// venue REST egress ceiling without re-parsing.
///
/// `pub(crate)` so the maker requote-budget bridge
/// ([`crate::bolt_v3_maker_rate_budget`]) sources its submit-governor cap and
/// window from the same single parser the config validator uses, rather than
/// introducing a second rate-string interpretation.
pub(crate) fn validate_rate_limit_string(value: &str) -> Result<(u64, u64), String> {
    let (limit, interval) = value
        .split_once('/')
        .ok_or_else(|| "expected `limit/HH:MM:SS`".to_string())?;
    let limit = limit.parse::<u64>().map_err(|error| error.to_string())?;
    if limit == 0 {
        return Err("limit must be greater than zero".to_string());
    }

    let mut parts = interval.split(':');
    let mut next_part = |label: &str| -> Result<u64, String> {
        parts
            .next()
            .ok_or_else(|| format!("missing {label} component"))?
            .parse::<u64>()
            .map_err(|error| format!("{label}: {error}"))
    };
    let hours = next_part("hours")?;
    let minutes = next_part("minutes")?;
    let seconds = next_part("seconds")?;
    if parts.next().is_some() {
        return Err("expected `limit/HH:MM:SS`".to_string());
    }
    if minutes >= 60 {
        return Err("minutes must be less than 60".to_string());
    }
    if seconds >= 60 {
        return Err("seconds must be less than 60".to_string());
    }
    if hours == 0 && minutes == 0 && seconds == 0 {
        return Err("interval must be greater than zero".to_string());
    }

    // Checked so a large `hours` value returns an Err instead of panicking
    // (debug) or wrapping to a bogus/zero interval (release). `minutes` is
    // bounded < 60 above so `minutes * SECONDS_PER_MINUTE` cannot overflow, but
    // it is kept inside the checked chain for a single readable expression.
    let interval_seconds = hours
        .checked_mul(SECONDS_PER_HOUR)
        .and_then(|h| h.checked_add(minutes * SECONDS_PER_MINUTE))
        .and_then(|s| s.checked_add(seconds))
        .ok_or_else(|| "interval seconds overflow u64".to_string())?;
    Ok((limit, interval_seconds))
}

/// Reconciles the global NT RiskEngine order submit/modify throttle against the
/// tightest configured trading-venue REST egress ceiling, derated by the venue's
/// worst-case per-order-command REST request fanout.
///
/// The RiskEngine throttle counts order *commands* while the venue HTTP quota
/// counts REST *requests*, and a single command can issue more than one request
/// (a Polymarket market quote-quantity BUY submit = collateral balance + book +
/// post = 3). A submit rate at the raw per-minute cap therefore over-drives the
/// venue's request quota by the fanout factor; the excess does not reject early
/// with a loud `OrderDenied` — it blocks at egress (added latency, stale
/// reference quotes), a silent failure on a live-money path. Reconciling `limit *
/// fanout` against the cap at config load keeps the policy fail-loud regardless
/// of the rendered deploy-time value, which is not otherwise knowable from the
/// repo.
///
/// NOTE (tier-1): this derates submit/modify against the per-bucket ceiling using
/// the deterministic worst-case per-command fanout only. The full shared REST
/// budget — transient retries, cancels, status queries, readiness/account probes,
/// and the fact that CLOB and Gamma are *separate* per-client buckets — is the
/// venue egress-capability contract tracked in #501.
pub(super) fn validate_order_rate_within_venue_egress(root: &BoltV3RootConfig) -> Vec<String> {
    let mut errors = Vec::new();
    // Fail closed on any execution venue whose REST egress model bolt-v3 does not
    // model: skipping it silently would let an unbounded submit rate through on a
    // venue we cannot reconcile against. Iterate the keyed client map so the
    // error can name the offending `clients.<id>`.
    let mut tightest: Option<(&str, crate::bolt_v3_providers::VenueEgressModel)> = None;
    for (key, client) in &root.clients {
        if client.execution.is_none() {
            continue;
        }
        let venue = client.venue.as_str();
        match crate::bolt_v3_providers::venue_egress_model(venue) {
            Some(model) => {
                // Tightest = smallest effective ceiling cap/fanout. Compare via
                // cross-multiplication (cap_a/fanout_a < cap_b/fanout_b iff
                // cap_a * fanout_b < cap_b * fanout_a) in u128 to avoid integer
                // division and any saturation.
                let tighter = tightest.is_none_or(|(_, current)| {
                    (model.cap_per_minute as u128)
                        * (current.max_rest_requests_per_order_command as u128)
                        < (current.cap_per_minute as u128)
                            * (model.max_rest_requests_per_order_command as u128)
                });
                if tighter {
                    tightest = Some((venue, model));
                }
            }
            None => errors.push(format!(
                "clients.{key} (provider={venue}) declares an [execution] block but bolt-v3 \
                 models no REST egress cap for it; cannot reconcile order rates — fail closed"
            )),
        }
    }
    let Some((venue, model)) = tightest else {
        // No modeled execution venue to reconcile against; `errors` may already
        // carry fail-closed messages for unmodeled execution venues above.
        return errors;
    };
    let cap_per_minute = model.cap_per_minute;
    let fanout = model.max_rest_requests_per_order_command;
    // Largest order-command rate per minute that keeps `limit * fanout <= cap`.
    let derated_ceiling = cap_per_minute / fanout;
    for (label, value) in [
        (
            "risk.nautilus.max_order_submit_rate",
            root.risk.nautilus.max_order_submit_rate.as_str(),
        ),
        (
            "risk.nautilus.max_order_modify_rate",
            root.risk.nautilus.max_order_modify_rate.as_str(),
        ),
    ] {
        // Only well-formed rate strings reach the ceiling check; malformed
        // strings are already reported by validate_rate_limit_string above.
        let Ok((limit, interval_seconds)) = validate_rate_limit_string(value) else {
            continue;
        };
        // Over-drives the cap iff limit/interval > (cap/fanout)/60, i.e.
        // limit * fanout * SECONDS_PER_MINUTE > cap * interval_seconds. Compared
        // in u128 so no product can saturate to u64::MAX and let an over-cap rate
        // slip through (MAX > MAX is false). validate_rate_limit_string guarantees
        // interval_seconds >= 1, so no zero-interval guard is needed.
        if (limit as u128) * (fanout as u128) * (SECONDS_PER_MINUTE as u128)
            > (cap_per_minute as u128) * (interval_seconds as u128)
        {
            errors.push(format!(
                "{label} = `{value}` over-drives the {venue} REST egress cap of \
                 {cap_per_minute}/min (nautilus HTTP_RATE_LIMIT): a single order command issues up \
                 to {fanout} REST requests (market quote-quantity BUY submit = balance + book + \
                 post), so the order rate must not exceed {derated_ceiling}/min or submits block \
                 at egress with stale reference quotes instead of failing loud — lower it to at \
                 most {derated_ceiling}/00:01:00"
            ));
        }
    }
    errors
}
