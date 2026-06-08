# Data Model: Global RV Surface Runtime

## RealizedVolSurfaceRuntime

Runtime-level owner of all configured realized-volatility surfaces.

Fields:
- `surfaces: BTreeMap<String, RealizedVolSurfaceState>` keyed by `surface_id`
- `subscriptions: BTreeMap<PhysicalSubscriptionKey, Vec<RealizedVolSourceRoute>>`
- `latest_snapshots: BTreeMap<String, RealizedVolSnapshot>` or equivalent snapshot access path
- `unknown_route_rejections` for runtime-level routing failures that are not valid configured source IDs

Rules:
- One runtime instance per live node/runtime context.
- No strategy owns this state.
- Snapshot lookup is by opaque `surface_id`.
- Observation routing fan-outs configured routes without duplicating physical subscriptions.

## RealizedVolSurfaceState

Runtime state for one configured surface.

Fields:
- `surface_id`
- `engine: RealizedVolEngine`
- `source_routes: Vec<RealizedVolSourceRoute>`
- `latest_snapshot: Option<RealizedVolSnapshot>`
- `config_fingerprint`

Rules:
- Owns one engine per surface, not per strategy.
- Builds from root TOML only.
- Publishes snapshots for all consumers.

## RealizedVolSourceRoute

TOML-derived route from a physical market-data stream to a logical RV source.

Fields:
- `surface_id`
- `source_id`
- `data_client_id`
- `instrument_id`
- `source_class`
- `sample_kind`
- `enabled`
- `counts_toward_quorum`

Rules:
- Disabled sources may remain in diagnostics but should not create unnecessary live subscriptions unless audit fan-out requires it.
- Multiple logical sources can bind one physical stream and receive fan-out.

## PhysicalSubscriptionKey

Deduplicated market-data subscription identity.

Fields:
- `data_client_id`
- `instrument_id`
- `subscription_kind`: quote, trade, index, mark when supported

Rules:
- One physical subscription per key.
- Routes define which surfaces/source IDs receive observations from that key.

## RealizedVolObservation

Normalized input to RV engine.

Existing fields remain:
- `source_id`
- `source_class`
- `sample_kind`
- `price`
- `event_ts_ms`
- `recv_ts_ms`

Rules:
- Created by runtime routing from NT market-data events.
- Strategy code must not construct production RV observations.

## Future HorizonRvEstimate

Per-source, per-horizon RV estimate. This is future scope; PR #615 computes one configured fixed-grid window per source and publishes an empty `horizon_estimates` placeholder.

Fields:
- `horizon_id`
- `window_ms`
- `sampling_interval_ms`
- `estimator_method`
- `annualized_realized_vol_decimal`
- `coverage_ratio`
- `valid_grid_count`
- `max_inter_sample_gap_ms`
- `block_reason`

Rules:
- Zero RV is valid.
- Missing required horizon behavior follows TOML final policy.

## SourceRobustRvEstimate

Per-source final estimate after measured fixed-grid RV, noise handling, and jump diagnostics.

Fields:
- `source_id`
- `horizon_estimates` future placeholder
- `continuous_rv_decimal`
- `jump_component_decimal`
- `measured_rv_decimal`
- `noise_robust_rv_decimal`
- `forecast_rv_decimal` future placeholder
- `final_source_rv_decimal`
- `estimator_diagnostics`

Rules:
- Does not hide jump contribution.
- Final policy is explicit and TOML-owned.

## SurfaceRobustAggregation

Cross-source aggregation result.

Fields:
- `sources_used`
- `source_estimates`
- `aggregation_method`
- `aggregate_rv_decimal`
- `dispersion_ratio`
- `mad_ratio` optional
- `blocked_reasons`

Rules:
- Aggregates only eligible ready source estimates.
- One outlier source must not silently dominate when robust aggregation is configured.

## RealizedVolSnapshot

Public payload consumed by pricing, maker, evidence, and future consumers.

Existing fields remain and are extended with:
- future per-horizon estimate placeholder
- measured/noise-robust/continuous/jump/final RV fields where configured, plus a future forecast placeholder
- runtime provenance and config fingerprint
- robust aggregation diagnostics

Rules:
- Consumers must use ready snapshot accessors.
- Snapshot is the only consumer contract; engine internals remain private.
