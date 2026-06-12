# Evidence Schema Contract: Robust RV Runtime

## Purpose

Evidence must prove which global RV surface was consumed, why it was ready or blocked, which sources contributed, what measured/noise-robust/jump/aggregation outputs were published, and which final RV value pricing used.

## Versioning

The baseline after PR #609 is decision-evidence schema v8. Feature 027 must bump the schema to v9 unless another intervening merged change has already advanced the version, in which case feature 027 bumps from that new baseline by exactly one version.

Adding global runtime provenance, noise-robust estimates, jump components, robust aggregation provenance, or future multi-horizon/forecast components requires a decision-evidence schema change. PR #615 bumps to v9 for global runtime and measured/noise-robust/continuous/jump component fields.

Consumers must tolerate unknown fields for forward compatibility, but the schema version must signal every material format change.

## Required Snapshot Fields

Every decision evidence snapshot that references RV must include:

- `realized_volatility_surface_id`
- `realized_volatility_as_of_ms`
- `realized_volatility_annualized_decimal`
- `realized_volatility_pricing_component`
- `realized_volatility_measured_annualized_decimal`
- `realized_volatility_noise_robust_annualized_decimal`
- `realized_volatility_continuous_annualized_decimal`
- `realized_volatility_jump_annualized_decimal`
- `realized_volatility_forecast_annualized_decimal`
- `realized_volatility_seconds_per_annum`
- `realized_volatility_aggregation`
- `realized_volatility_sources_used`
- `realized_volatility_source_diagnostics`
- `realized_volatility_blockers`
- `realized_volatility_unknown_source_rejections`
- `realized_volatility_config_fingerprint`

Empty numeric fields mean no value was published for that component. They must not be backfilled from unrelated fallback values. A valid zero RV must be serialized as a numeric zero string, not as an empty field. Forecast fields remain empty in PR #615 because forecast pricing is rejected by validation.

## Future Horizon Estimate Fields

Per-horizon evidence is future scope. PR #615 does not emit per-horizon evidence because it computes one configured fixed-grid window per source.

When multi-horizon ships, each horizon estimate must include:

Future fields:

- `horizon_name`
- `window_ms`
- `sampling_interval_ms`
- `weight`
- `required`
- `ready`
- `coverage_ratio`
- `sample_count`
- `expected_return_count`
- `valid_return_count`
- `base_fixed_grid_rv_decimal`
- `noise_robust_rv_decimal`
- `continuous_rv_decimal`
- `jump_rv_decimal`
- `ready_subsample_count`
- `attempted_subsample_count`
- `block_reason`

Rules:

- Required horizon blockers can block a source estimate.
- Optional horizon blockers are diagnostic unless final policy requires them.
- Zero RV is a valid value and must survive serialization/deserialization.
- Non-finite and negative RV values must never appear in evidence as usable values.

## Source Diagnostic Fields

Each source diagnostic must include:

- `source_id`
- `enabled`
- `counts_toward_quorum`
- `source_class`
- `sample_kind`
- `status`
- `last_rejected_reason`
- `rejection_counters`
- `annualized_realized_volatility_decimal`
- `measured_annualized_realized_volatility_decimal`
- `noise_robust_annualized_realized_volatility_decimal`
- `continuous_annualized_realized_volatility_decimal`
- `jump_annualized_realized_volatility_decimal`
- `first_sample_ts_ms`
- `last_sample_ts_ms`
- `raw_sample_count`
- `grid_sample_count`
- `coverage_ratio`
- `max_inter_sample_gap_ms`
- `block_reason`

Status labels:

- `ready`
- `blocked`
- `diagnostic_only`
- `waiting`

Rules:

- A source with `status = "ready"` must have `block_reason` empty.
- Historical rejection counters remain auditable but do not become current blockers after recovery.
- Disabled and non-quorum sources remain visible.
- Source-level blockers do not leak into surface blockers when quorum is satisfied.

## Surface Blocker Fields

Allowed surface blockers:

- `annualization_basis_invalid`
- `quorum_not_ready`
- `source_class_mismatch`
- `sample_kind_mismatch`
- `cross_source_dispersion`
- Future MAD/multi-horizon/forecast blockers must be added only when those slices ship.

Rules:

- Surface blockers describe why the final surface value is not usable.
- Per-source stale/not-warm/coverage/gap reasons stay in source diagnostics unless they make quorum short.
- When quorum is short, evidence records `quorum_not_ready` rather than copying every source-level reason into the surface blocker set.

## Pricing Value Provenance

Evidence must state which RV component pricing consumed:

- `measured`
- `noise_robust`
- `continuous`
- `forecast` is reserved but rejected by PR #615 validation.

The consumed component must correspond to the final `ReadyRealizedVol` value returned by the runtime accessor. Consumers must not recompute or substitute another component locally.

## Unknown Source Diagnostics

Unknown source rejection counters are emitted as a map of unknown source ID to count:

Rules:

- In PR #615, unknown source IDs are produced only by configured runtime routes, so the strategy/router boundary caps practical cardinality.
- Any future raw external ingestion path must add a TOML-owned or hardcoded-safe maximum cardinality before merge.

## Backward Compatibility

Legacy fields may remain only as aliases when necessary for existing evidence readers. They must be populated from the runtime snapshot, not from a strategy-owned estimator or fallback scalar.
