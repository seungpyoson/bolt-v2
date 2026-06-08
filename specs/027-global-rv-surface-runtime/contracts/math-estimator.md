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

Coverage ratio:

- `expected_return_count = floor(W / dt)`.
- `valid_return_count` is the count of adjacent grid-price pairs that produce valid positive finite log returns and pass source-age/gap constraints.
- `coverage_ratio = valid_return_count / expected_return_count`.
- If `expected_return_count == 0`, the horizon is invalid at config validation.

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

- Use `primary_horizon_name` and `floor_horizon_name` from TOML.
- Final source volatility is `max(primary_horizon, floor_horizon)`.

Short with long floor:

- Use `short_horizon_name`, `long_horizon_name`, and `floor_multiplier` from TOML.
- Final source volatility is `max(short_horizon, long_horizon * floor_multiplier)`.

## Microstructure-Noise Robust Modes

### none

Use base fixed-grid RV.

### coarser_grid

Compute the same base estimator using `coarse_sampling_interval_ms`. Emit both base and coarser estimates.

Final horizon value depends on `coarser_grid_horizon_policy`:

- `base`: use base fixed-grid estimate.
- `coarse`: use coarser estimate.
- `min_base_coarse`: use `min(base, coarse)` for noise suppression while retaining both in evidence.

### subsampled

For `k = subsamples`:

1. Create `k` deterministic offset grids within the same horizon.
2. For each `j` in `0..k`, offset `j` starts at `window_start + floor(j * sampling_interval_ms / k)`. Distinct integer millisecond offsets are required unless TOML explicitly enables offset-collision reporting.
3. Compute base fixed-grid RV for each offset grid.
4. Each offset grid computes coverage against its own expected return count after the offset start.
5. The subsampled estimate is the arithmetic mean of ready offset-grid RVs.
6. Reject unless `ready_offset_grid_count >= min_ready_subsamples`.

Evidence must include base fixed-grid RV, number of ready subsamples, number of attempted subsamples, per-offset coverage ratios, and subsampled RV.

## Jump Separation

The first implementation separates jumps instead of removing them.

For returns `r_1..r_n` where `n >= 2`:

- Measured variance: `RV = sum_i r_i^2`.
- Bipower proxy with finite-sample correction: `BV = (pi / 2) * (n / (n - 1)) * sum_{i=2..n} |r_i| * |r_{i-1}|`.
- Continuous variance: `continuous_var = min(RV, BV)`.
- Jump variance: `jump_var = max(RV - continuous_var, 0)`.

Annualize continuous and jump components using the same horizon annualization basis. Convert variance components to volatility components by square root after annualization.

Rules:

- The final pricing component is explicit in TOML and evidence.
- Jump variance is never silently discarded.
- If there are fewer than two returns, jump separation is diagnostic-only and cannot create a usable jump component.
- Flat valid prices produce zero measured, zero continuous, and zero jump components.

## Cross-Source Aggregation

Eligible contributors are enabled, quorum-counting, ready source estimates with valid source volatility.

### upper_quantile

Sort contributor volatilities ascending and select nearest-rank quantile `q` in `[0.5, 1.0]` using `index = ceil(q * n) - 1` on the zero-indexed sorted contributor list. Quantile selection is undefined for `n = 0`; the engine must block with `QuorumNotReady` before aggregation when there are zero eligible contributors.

### median

Select the median contributor volatility. For even counts, use the arithmetic mean of the two middle values.

### trimmed_mean

Sort contributors, remove configured symmetric `trim_fraction`, then average remaining values. Reject if trimming removes all contributors or leaves fewer than `min_ready_sources`.

### median_with_upper_quantile_guard

Compute median and the selected upper-quantile value. Final value is `max(median, upper_quantile_value * guard_weight)` where `guard_weight` is TOML-owned and non-negative.

## Dispersion and MAD Blocking

Relative dispersion:

- `dispersion = (max - min) / aggregate` when aggregate is positive.
- If aggregate is zero and any contributor is positive, dispersion is infinite.
- If aggregate is zero and all contributors are zero, dispersion is zero and does not block.
- Block if `dispersion > max_cross_source_dispersion`.

MAD dispersion:

- `median_abs_dev = median(|source_i - median(source_i over eligible contributors)|)`.
- `mad_ratio = median_abs_dev / median` when median is positive.
- If median is zero and any deviation is positive, MAD ratio is infinite.
- Block if `mad_ratio > mad_block_threshold`.

The first implementation intentionally uses raw MAD, not normal-calibrated `1.4826 * MAD`. TOML thresholds are calibrated against raw MAD ratio.

Dispersion blockers are surface-level blockers. Source-level readiness blockers stay in source diagnostics.

## Forecast Modes

Forecast modes are deterministic and evidence-first. Forecast state is owned per `surface_id` after cross-source aggregation and after the configured component selection that feeds the forecast.

### none

Final forecast component is absent. Pricing uses the configured measured/continuous/jump-adjusted/blended component.

### ewma

`forecast_t = alpha * current_component + (1 - alpha) * previous_forecast`

Rules:

- `alpha` is TOML-owned in `(0, 1]`.
- `alpha` is a refresh-step coefficient, not time-normalized in the first implementation.
- EWMA advances only on runtime `refresh(now_ms)` when the refresh publishes a ready current component, including when the numeric component equals the previous input, and `now_ms > previous_forecast_update_ms`.
- It does not advance on every observation.
- If no previous forecast exists, including after process restart, initialize from current component and record `forecast_cold_start = true` in evidence.
- Changing forecast config changes the surface config fingerprint and resets forecast state.
- Evidence records `alpha`, current component, previous forecast, final forecast, update timestamp, and cold-start flag.

### har_lite

`forecast = beta_short * short + beta_medium * medium + beta_long * long + intercept`

Rules:

- Betas and intercept are TOML-owned. Betas must be non-negative finite with sum in `(0, 1]`; intercept must be non-negative finite.
- Short, medium, and long role bindings must reference configured horizons.
- Referenced horizons must be ready.
- HAR-lite output must pass the shared valid-RV constructor before publication; non-finite output blocks the forecast component.
- Evidence records every input, coefficient, and whether the forecast component was blocked by numeric validation.

## Final Pricing Value

The final `ReadyRealizedVol` value is selected by TOML `pricing_component` from one of:

- measured multi-horizon blend
- noise-robust multi-horizon blend
- continuous component
- jump-adjusted component
- forecast component

Evidence must record the selected component and all intermediate components needed to reproduce it.
