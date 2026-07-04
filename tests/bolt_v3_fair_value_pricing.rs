use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_fair_value_pricing::{
        FairValuePricingBlockReason, FairValuePricingConfig, FairValuePricingRequest,
        FairValuePricingState, FastSpotObservation,
    },
    bolt_v3_realized_volatility::{
        RealizedVolAggregation, RealizedVolBlockReason, RealizedVolPricingComponent,
        RealizedVolSnapshot,
    },
    bolt_v3_timestamp_domain::VenueEventMs,
};

const TARGET_SURFACE_ID: &str = "<TARGET_SURFACE_ID>";
const OTHER_SURFACE_ID: &str = "<OTHER_SURFACE_ID>";
const TARGET_SOURCE_ID: &str = "<TARGET_SOURCE_ID>";
const OTHER_SOURCE_ID: &str = "<OTHER_SOURCE_ID>";
const TARGET_READY_TS_MS: u64 = 1_000;
const OTHER_NEWER_TS_MS: u64 = 2_000;
const TARGET_REALIZED_VOL: f64 = 1.5;
const OTHER_REALIZED_VOL: f64 = 3.0;

fn pricing_config() -> FairValuePricingConfig<'static> {
    FairValuePricingConfig {
        realized_volatility_surface_id: TARGET_SURFACE_ID,
        realized_volatility_max_source_age_ms: None,
        pricing_kurtosis: 0.0,
        market_family: "updown",
    }
}

fn pricing_request() -> FairValuePricingRequest {
    FairValuePricingRequest {
        now_ms: TARGET_READY_TS_MS,
        realized_vol_gate_event_ms: Some(VenueEventMs::new(TARGET_READY_TS_MS)),
        strike_price: Some(3_100.0),
        seconds_to_market_end: Some(300),
    }
}

fn ready_snapshot(surface_id: &str, as_of_ms: u64, realized_vol: f64) -> RealizedVolSnapshot {
    snapshot(
        surface_id,
        as_of_ms,
        realized_vol,
        true,
        &[TARGET_SOURCE_ID],
    )
}

fn unready_snapshot(surface_id: &str, as_of_ms: u64, realized_vol: f64) -> RealizedVolSnapshot {
    snapshot(
        surface_id,
        as_of_ms,
        realized_vol,
        false,
        &[OTHER_SOURCE_ID],
    )
}

fn snapshot(
    surface_id: &str,
    as_of_ms: u64,
    realized_vol: f64,
    ready: bool,
    sources_used: &[&str],
) -> RealizedVolSnapshot {
    RealizedVolSnapshot {
        surface_id: surface_id.to_string(),
        as_of_ms,
        annualized_realized_vol_decimal: Some(realized_vol),
        measured_annualized_realized_vol_decimal: Some(realized_vol),
        noise_robust_annualized_realized_vol_decimal: Some(realized_vol),
        continuous_annualized_realized_vol_decimal: Some(realized_vol),
        jump_annualized_realized_vol_decimal: Some(0.0),
        forecast_annualized_realized_vol_decimal: None,
        pricing_component: RealizedVolPricingComponent::Measured,
        ready,
        sources_used: sources_used
            .iter()
            .map(|source| (*source).to_string())
            .collect(),
        source_diagnostics: Vec::new(),
        horizon_estimates: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: if ready {
            Vec::new()
        } else {
            vec![RealizedVolBlockReason::QuorumNotReady]
        },
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: String::new(),
    }
}

fn observe_ready_fair_value_state() -> FairValuePricingState {
    let mut state =
        FairValuePricingState::from_realized_volatility_surface_id(TARGET_SURFACE_ID.into());
    state.observe_pricing_spot(&FastSpotObservation {
        venue: "<SPOT_SOURCE_ID>".to_string(),
        price: 3_105.0,
        observed_ts_ms: TARGET_READY_TS_MS,
        received_ts_ms: None,
    });
    state.observe_realized_vol_snapshot(ready_snapshot(
        TARGET_SURFACE_ID,
        TARGET_READY_TS_MS,
        TARGET_REALIZED_VOL,
    ));
    state
}

#[test]
fn different_surface_snapshot_does_not_evict_active_realized_volatility() {
    let mut state =
        FairValuePricingState::from_realized_volatility_surface_id(TARGET_SURFACE_ID.into());
    state.observe_realized_vol_snapshot(ready_snapshot(
        TARGET_SURFACE_ID,
        TARGET_READY_TS_MS,
        TARGET_REALIZED_VOL,
    ));
    state.observe_realized_vol_snapshot(ready_snapshot(
        OTHER_SURFACE_ID,
        OTHER_NEWER_TS_MS,
        OTHER_REALIZED_VOL,
    ));

    assert_eq!(
        state
            .latest_realized_vol_snapshot_for_surface(TARGET_SURFACE_ID)
            .map(|snapshot| snapshot.surface_id.as_str()),
        Some(TARGET_SURFACE_ID)
    );
    assert_eq!(
        state.current_realized_vol_at(Some(VenueEventMs::new(OTHER_NEWER_TS_MS)), None),
        Some(TARGET_REALIZED_VOL)
    );
    // Differential vs #755 filter-at-observe: the foreign snapshot is RETAINED under its own
    // key — the map behavior #770 prescribes. A filter-at-observe impl would have dropped it
    // at write time, so this assertion fails under the filter and proves the map is
    // load-bearing here, not just an equivalent reformulation.
    assert_eq!(
        state
            .latest_realized_vol_snapshot_for_surface(OTHER_SURFACE_ID)
            .map(|snapshot| snapshot.surface_id.as_str()),
        Some(OTHER_SURFACE_ID)
    );
}

#[test]
fn fair_value_inputs_report_all_missing_input_blockers_in_stable_order() {
    let state =
        FairValuePricingState::from_realized_volatility_surface_id(TARGET_SURFACE_ID.into());

    let blocked_by = state
        .fair_value_inputs_at(
            &pricing_config(),
            FairValuePricingRequest {
                now_ms: TARGET_READY_TS_MS,
                realized_vol_gate_event_ms: Some(VenueEventMs::new(TARGET_READY_TS_MS)),
                strike_price: None,
                seconds_to_market_end: None,
            },
        )
        .expect_err("missing fair-value inputs should block pricing");

    assert_eq!(
        blocked_by,
        vec![
            FairValuePricingBlockReason::SpotPriceMissing,
            FairValuePricingBlockReason::StrikePriceMissing,
            FairValuePricingBlockReason::SecondsToExpiryMissing,
            FairValuePricingBlockReason::RealizedVolNotReady,
        ]
    );
}

#[test]
fn fair_value_pricing_reports_realized_vol_source_evidence() {
    let state = observe_ready_fair_value_state();

    let result = state
        .fair_value_pricing_at(&pricing_config(), pricing_request())
        .expect("ready fair-value state should price");

    assert_eq!(result.spot_price, 3_105.0);
    assert_eq!(result.strike_price, 3_100.0);
    assert_eq!(result.seconds_to_market_end, 300);
    assert_eq!(result.realized_vol, TARGET_REALIZED_VOL);
    assert_eq!(
        result.realized_vol_surface_id.as_deref(),
        Some(TARGET_SURFACE_ID)
    );
    assert_eq!(
        result.realized_vol_source_venue.as_deref(),
        Some(TARGET_SOURCE_ID)
    );
    assert_eq!(result.realized_vol_source_ts_ms, Some(TARGET_READY_TS_MS));
    assert!(result.fair_probability_up.is_finite());
    assert_eq!(
        result.fair_probability_down,
        1.0 - result.fair_probability_up
    );
}

#[test]
fn fair_value_pricing_reports_no_source_venue_when_snapshot_sources_are_empty() {
    let mut state =
        FairValuePricingState::from_realized_volatility_surface_id(TARGET_SURFACE_ID.into());
    state.observe_pricing_spot(&FastSpotObservation {
        venue: "<SPOT_SOURCE_ID>".to_string(),
        price: 3_105.0,
        observed_ts_ms: TARGET_READY_TS_MS,
        received_ts_ms: None,
    });
    state.observe_realized_vol_snapshot(snapshot(
        TARGET_SURFACE_ID,
        TARGET_READY_TS_MS,
        TARGET_REALIZED_VOL,
        true,
        &[],
    ));

    let result = state
        .fair_value_pricing_at(&pricing_config(), pricing_request())
        .expect("ready fair-value state should price without source attribution");

    assert_eq!(result.realized_vol_source_venue, None);
    assert_eq!(result.realized_vol_source_ts_ms, Some(TARGET_READY_TS_MS));
    assert_eq!(
        state.current_realized_vol_source_at(Some(VenueEventMs::new(TARGET_READY_TS_MS)), None),
        (None, Some(TARGET_READY_TS_MS))
    );
}

#[test]
fn newer_same_surface_unready_snapshot_blocks_pricing_fail_closed() {
    let mut state = observe_ready_fair_value_state();
    state.observe_realized_vol_snapshot(unready_snapshot(
        TARGET_SURFACE_ID,
        OTHER_NEWER_TS_MS,
        OTHER_REALIZED_VOL,
    ));

    assert_eq!(
        state
            .latest_realized_vol_snapshot_for_surface(TARGET_SURFACE_ID)
            .map(|snapshot| (
                snapshot.surface_id.as_str(),
                snapshot.as_of_ms,
                snapshot.ready
            )),
        Some((TARGET_SURFACE_ID, OTHER_NEWER_TS_MS, false))
    );
    assert_eq!(
        state.current_realized_vol_at(Some(VenueEventMs::new(OTHER_NEWER_TS_MS)), None),
        None
    );

    let blocked_by = state
        .fair_value_inputs_at(
            &pricing_config(),
            FairValuePricingRequest {
                now_ms: OTHER_NEWER_TS_MS,
                realized_vol_gate_event_ms: Some(VenueEventMs::new(OTHER_NEWER_TS_MS)),
                ..pricing_request()
            },
        )
        .expect_err("newer unready snapshot should block instead of using stale ready RV");

    assert_eq!(
        blocked_by,
        vec![FairValuePricingBlockReason::RealizedVolNotReady]
    );
}
