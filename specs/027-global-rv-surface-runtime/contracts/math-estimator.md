# Math Estimator Contract: Robust RV Surfaces

## Numeric Contract

All usable RV values must pass through the shared valid-RV constructor:

- finite
- non-negative
- zero is valid
- negative zero is normalized to zero for serialization/evidence

Consumers must use the ready accessor and must not revalidate raw `f64` values.

## Base Fixed-Grid RV

For a horizon with window `W`, sampling interval `dt`, and annualization basis `A`:

1. Build deterministic grid timestamps from `as_of_ms - W` to `as_of_ms`.
2. Fill grid prices by LOCF from accepted observations, respecting source age and inter-sample gap constraints.
3. Compute log returns `r_i = ln(p_i / p_{i-1})` for positive finite adjacent prices.
4. Compute realized variance `RV = sum_i r_i^2`.
5. Annualize as `annualized_rv = RV * A / horizon_seconds`.
6. Annualized realized volatility is `sqrt(annualized_rv)`.

Flat valid prices produce zero RV and zero volatility.

## Multi-Horizon Blend

Each source emits one estimate per configured horizon.

Required horizons:

- If a required horizon is not ready, the source estimate is blocked with `RequiredHorizonNotReady`.

Optional horizons:

- Optional horizon failures remain diagnostic unless selected final policy says otherwise.

Weighted blend:

- Let ready selected horizons be `h_i` with weights `w_i`.
- Reject if `sum(w_i) <= 0`.
- Final source volatility is `sum(w_i * h_i) / sum(w_i)`.

Max floor:

- Final source volatility is `max(primary_horizon, floor_horizon)`.

Short with long floor:

- Final source volatility is `max(short_horizon, long_horizon * floor_multiplier)`.

## Microstructure-Noise Robust Modes

### none

Use base fixed-grid RV.

### coarser_grid

Compute the same base estimator using `coarse_sampling_interval_ms`. Emit both base and coarser estimates.

Final horizon value depends on TOML policy:

- `base`: use base fixed-grid estimate.
- `coarse`: use coarser estimate.
- `min_base_coarse`: use `min(base, coarse)` for noise suppression while retaining both in evidence.

### subsampled

For `k = subsamples`:

1. Create `k` deterministic offset grids within the same horizon.
2. Offset `j` starts at `window_start + floor(j * sampling_interval_ms / k)`.
3. Compute base fixed-grid RV for each offset grid.
4. The subsampled estimate is the arithmetic mean of ready offset-grid RVs.
5. Reject if no offset grid is ready.

Evidence must include base fixed-grid RV, number of ready subsamples, and subsampled RV.

## Jump Separation

The first implementation separates jumps instead of removing them.

For returns `r_1..r_n`:

- Measured variance: `RV = sum_i r_i^2`.
- Bipower proxy: `BV = (pi / 2) * sum_{i=2..n} |r_i| * |r_{i-1}|`.
- Continuous variance: `continuous_var = min(RV, BV)`.
- Jump variance: `jump_var = max(RV - continuous_var, 0)`.

Annualize continuous and jump components using the same horizon annualization basis. Convert variance components to volatility components by square root after annualization.

Rules:

- The final pricing component is explicit in TOML and evidence.
- Jump variance is never silently discarded.
- If there are fewer than two returns, jump separation is diagnostic-only and cannot create a usable jump component.

## Cross-Source Aggregation

Eligible contributors are enabled, quorum-counting, ready source estimates with valid source volatility.

### upper_quantile

Sort contributor volatilities ascending and select nearest-rank quantile `q` in `[0.5, 1.0]`.

### median

Select the median contributor volatility. For even counts, use the arithmetic mean of the two middle values.

### trimmed_mean

Sort contributors, remove configured symmetric trim count or fraction, then average remaining values. Reject if trimming removes all contributors or leaves fewer than `min_ready_sources`.

### median_with_upper_quantile_guard

Compute median and upper quantile. Final value is `max(median, upper_quantile * guard_weight)` where `guard_weight` is TOML-owned and non-negative.

## Dispersion and MAD Blocking

Relative dispersion:

- `dispersion = (max - min) / aggregate` when aggregate is positive.
- If aggregate is zero and any contributor is positive, dispersion is infinite.
- Block if `dispersion > max_cross_source_dispersion`.

MAD dispersion:

- `median_abs_dev = median(|source_i - median(source)|)`.
- `mad_ratio = median_abs_dev / median` when median is positive.
- If median is zero and any deviation is positive, MAD ratio is infinite.
- Block if `mad_ratio > mad_block_threshold`.

Dispersion blockers are surface-level blockers. Source-level readiness blockers stay in source diagnostics.

## Forecast Modes

Forecast modes are deterministic and evidence-first.

### none

Final forecast component is absent. Pricing uses the configured measured/continuous/jump-adjusted/blended component.

### ewma

`forecast_t = alpha * current_component + (1 - alpha) * previous_forecast`

Rules:

- `alpha` is TOML-owned in `(0, 1]`.
- If no previous forecast exists, initialize from current component.
- Evidence records `alpha`, current component, previous forecast, and final forecast.

### har_lite

`forecast = beta_short * short + beta_medium * medium + beta_long * long + intercept`

Rules:

- Betas and intercept are TOML-owned.
- Required referenced horizons must be ready.
- Evidence records every input and coefficient.

## Final Pricing Value

The final `ReadyRealizedVol` value is selected by TOML policy from one of:

- measured multi-horizon blend
- noise-robust multi-horizon blend
- continuous component
- jump-adjusted component
- forecast component

Evidence must record the selected component and all intermediate components needed to reproduce it.
