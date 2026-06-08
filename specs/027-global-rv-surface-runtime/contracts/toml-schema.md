# TOML Schema Contract: Global RV Surfaces

## Root Ownership

All RV runtime values live under root TOML. Strategy TOMLs may only select a surface by ID.

Strategies must not contain RV policy knobs such as windows, sampling intervals, aggregation policy, dispersion thresholds, forecast weights, noise filters, jump thresholds, or source routes.

## Surface Table

```toml
[realized_volatility_surfaces.<surface_id>]
canonical_base_asset = "<BASE_ASSET>"
refresh_interval_ms = 1000
seconds_per_annum = 31536000.0
min_ready_sources = 2
max_cross_source_dispersion = 0.35
single_source_explanation = "<REQUIRED_WHEN_SINGLE_SOURCE>"
```

Rules:

- `<surface_id>` is opaque, non-empty, trimmed, case-sensitive, and globally unique after trimming; two IDs that differ only by leading/trailing whitespace are duplicates.
- `canonical_base_asset` must match the referenced strategy target's underlying asset.
- `seconds_per_annum` must be positive finite.
- `refresh_interval_ms` must be positive.
- `min_ready_sources` must be positive and no greater than enabled quorum-counting sources.
- `max_cross_source_dispersion` must be non-negative finite.
- Shipped production surfaces with fewer than two enabled quorum-counting sources must set `single_source_explanation` to a trimmed, non-empty operator-facing reason no longer than 512 characters. This is documentation/evidence metadata, not a runtime policy escape hatch.

## Sources

```toml
[[realized_volatility_surfaces.<surface_id>.sources]]
source_id = "<SOURCE_ID>"
client_id = "<PUBLIC_DATA_CLIENT_ID>"
instrument_id = "<INSTRUMENT_ID>"
source_class = "spot_quote"
sample_kind = "midpoint"
enabled = true
counts_toward_quorum = true
max_source_age_ms = 5000
max_receive_lag_ms = 1000
```

Allowed source-class/sample-kind pairs for the initial production slice:

- `spot_quote` / `midpoint`
- `trade` / `trade`
- `index` / `index`

`mark` may remain a declared enum only if validation clearly rejects it before runtime construction, and runtime construction still fails loudly if validation is bypassed, until mark routing is implemented.

Rules:

- `source_id` is unique within a surface.
- `client_id` must reference a configured public market-data client.
- `instrument_id` must be valid for the referenced client.
- `instrument_id` must resolve to the same canonical base asset as the parent surface for that client/source class.
- Disabled sources remain in diagnostics but do not subscribe and never count toward quorum.
- Enabled `counts_toward_quorum = false` sources may subscribe and produce diagnostics but never contribute to readiness, aggregation, or surface blockers.

## Estimator Policy

```toml
[realized_volatility_surfaces.<surface_id>.estimator]
method = "multi_horizon"
final_policy = "weighted_blend"
pricing_component = "noise_robust"
noise_robust_method = "subsampled"
jump_policy = "separate"
forecast_method = "none"
primary_horizon_name = "short"
floor_horizon_name = "long"
short_horizon_name = "short"
medium_horizon_name = "medium"
long_horizon_name = "long"
floor_multiplier = 1.0
```

Allowed `method` values:

- `single_horizon_fixed_grid`
- `multi_horizon`

Allowed `final_policy` values:

- `weighted_blend`
- `max_floor`
- `short_with_long_floor`

Allowed `pricing_component` values:

- `measured`
- `noise_robust`
- `continuous`
- `jump_adjusted`
- `forecast`

Allowed `noise_robust_method` values for first implementation:

- `none`
- `coarser_grid`
- `subsampled`

Allowed `jump_policy` values:

- `none`
- `separate`

Allowed `forecast_method` values:

- `none`
- `ewma`
- `har_lite`

Rules:

- Horizon role names must reference configured horizon names when the selected final or forecast policy needs them.
- `floor_multiplier` must be non-negative finite when `final_policy = "short_with_long_floor"`.
- `pricing_component = "forecast"` requires `forecast_method != "none"`.
- `pricing_component = "noise_robust"` requires `noise_robust_method != "none"`.
- `forecast_method = "har_lite"` requires short, medium, and long role bindings.

## Horizons

```toml
[[realized_volatility_surfaces.<surface_id>.estimator.horizons]]
name = "short"
window_ms = 300000
sampling_interval_ms = 1000
weight = 0.50
min_coverage_ratio = 0.80
max_inter_sample_gap_ms = 10000
required = true

[[realized_volatility_surfaces.<surface_id>.estimator.horizons]]
name = "medium"
window_ms = 1800000
sampling_interval_ms = 5000
weight = 0.30
min_coverage_ratio = 0.75
max_inter_sample_gap_ms = 30000
required = true

[[realized_volatility_surfaces.<surface_id>.estimator.horizons]]
name = "long"
window_ms = 7200000
sampling_interval_ms = 15000
weight = 0.20
min_coverage_ratio = 0.70
max_inter_sample_gap_ms = 60000
required = false
```

Rules:

- Horizon names are unique within a surface.
- At least one horizon is required.
- `window_ms >= sampling_interval_ms`.
- `weight` is non-negative finite.
- Required horizon weights must sum to a positive finite value.
- `min_coverage_ratio` is in `(0, 1]`.
- `max_inter_sample_gap_ms >= sampling_interval_ms`.
- Missing optional horizons must not block surface readiness unless selected by final or forecast policy.

## Noise Robustness

```toml
[realized_volatility_surfaces.<surface_id>.estimator.noise]
subsamples = 3
min_ready_subsamples = 2
coarse_sampling_interval_ms = 5000
coarser_grid_horizon_policy = "coarse"
allow_subsample_offset_collisions = false
```

Allowed `coarser_grid_horizon_policy` values:

- `base`
- `coarse`
- `min_base_coarse`

Rules:

- `subsamples` is positive when `noise_robust_method = "subsampled"`.
- `min_ready_subsamples` is positive and no greater than `subsamples` when `noise_robust_method = "subsampled"`.
- `subsamples` must be no greater than the smallest selected horizon `sampling_interval_ms` when `allow_subsample_offset_collisions = false` so integer millisecond offsets remain unique.
- If `allow_subsample_offset_collisions = true`, evidence must report attempted and distinct offset counts.
- `coarse_sampling_interval_ms` is positive when `noise_robust_method = "coarser_grid"`.
- `coarser_grid_horizon_policy` is required when `noise_robust_method = "coarser_grid"`.
- Noise-robust mode must emit both base fixed-grid RV and noise-robust RV in diagnostics.

## Jump Separation

```toml
[realized_volatility_surfaces.<surface_id>.estimator.jump]
method = "bipower_variation"
threshold_multiplier = 4.0
```

Rules:

- Jump separation must not silently delete jumps from evidence.
- Snapshot output includes continuous component and jump component.
- Pricing policy decides whether to use measured, continuous, jump-adjusted, or forecast RV.
- Fewer than two returns makes jump separation diagnostic-only; it cannot publish a usable jump component.

## Forecast

```toml
[realized_volatility_surfaces.<surface_id>.estimator.forecast]
ewma_alpha = 0.30
har_intercept = 0.0
har_short_weight = 0.50
har_medium_weight = 0.30
har_long_weight = 0.20
```

Rules:

- Forecast state is owned per `surface_id` after cross-source aggregation and component selection.
- EWMA advances only on runtime `refresh(now_ms)` when a new ready current component is published and `now_ms` is greater than the previous forecast update timestamp.
- EWMA `alpha` is a refresh-step coefficient in `(0, 1]`, not time-normalized in the first implementation.
- If no previous forecast exists, including after process restart, initialize from the current ready component and record cold start in evidence.
- Changing forecast config changes the surface config fingerprint and resets forecast state.
- HAR-lite weights must be non-negative finite and their sum must be in `(0, 1]`.
- HAR-lite intercept must be non-negative finite.
- Referenced short/medium/long horizons must be ready.
- HAR-lite output must pass the shared valid-RV constructor before publication; non-finite output blocks the forecast component.

## Cross-Source Aggregation

```toml
[realized_volatility_surfaces.<surface_id>.aggregation]
method = "median_with_upper_quantile_guard"
upper_quantile = 0.75
guard_weight = 1.0
mad_block_threshold = 0.50
trim_fraction = 0.0
```

Allowed methods:

- `upper_quantile`
- `median`
- `trimmed_mean`
- `median_with_upper_quantile_guard`

Rules:

- `upper_quantile` is in `[0.5, 1.0]`.
- `guard_weight` is non-negative finite when `method = "median_with_upper_quantile_guard"`.
- `mad_block_threshold` is non-negative finite.
- `trim_fraction` is in `[0, 0.5)` when `method = "trimmed_mean"`.
- Aggregation uses eligible ready contributors only.
- Dispersion/MAD blockers are surface-level blockers.

## Strategy Selector

```toml
[strategies.<strategy_id>]
realized_volatility_surface_id = "<surface_id>"
```

Rules:

- Required for every binary oracle taker and maker strategy that needs RV.
- The referenced surface must exist in root TOML.
- No strategy-local legacy RV fields are permitted.

## Production Multi-Venue Rule

For shipped production surfaces, configure every available public venue/source that has a valid client and instrument mapping for the canonical asset. Availability means the root config contains an enabled public market-data client and a valid instrument mapping for that canonical asset/source class. If only one source is available, `single_source_explanation` must be trimmed, non-empty, no longer than 512 characters, and emitted in evidence; Rust code must not hardcode the limitation.
