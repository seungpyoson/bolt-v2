use std::collections::BTreeMap;

use bolt_v2::{
    bolt_v3_fair_value_pricing::FairValuePricingState,
    bolt_v3_realized_volatility::{
        RealizedVolAggregation, RealizedVolPricingComponent, RealizedVolSnapshot,
    },
};

const TARGET_SURFACE_ID: &str = "<TARGET_SURFACE_ID>";
const OTHER_SURFACE_ID: &str = "<OTHER_SURFACE_ID>";
const TARGET_READY_TS_MS: u64 = 1_000;
const OTHER_NEWER_TS_MS: u64 = 2_000;
const TARGET_REALIZED_VOL: f64 = 1.5;
const OTHER_REALIZED_VOL: f64 = 3.0;

fn ready_snapshot(surface_id: &str, as_of_ms: u64, realized_vol: f64) -> RealizedVolSnapshot {
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
        ready: true,
        sources_used: Vec::new(),
        source_diagnostics: Vec::new(),
        horizon_estimates: Vec::new(),
        unknown_source_rejections: BTreeMap::new(),
        blocked_reasons: Vec::new(),
        aggregate_method: RealizedVolAggregation::UpperQuantile { quantile: 1.0 },
        seconds_per_annum: 31_536_000.0,
        config_fingerprint: String::new(),
    }
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
            .latest_realized_vol_snapshot()
            .map(|snapshot| snapshot.surface_id.as_str()),
        Some(TARGET_SURFACE_ID)
    );
    assert_eq!(
        state.current_realized_vol_at(OTHER_NEWER_TS_MS),
        Some(TARGET_REALIZED_VOL)
    );
}
