# External Review Resolution Log

## Review Round 1

Reviewed bundle commit: `882321f7c708e83eb77f09cf16c2da76afb67ff6`

Models:

- Gemini: APPROVE with missing-test risks.
- Grok: APPROVE with non-blocking test specificity concerns.
- Claude: REQUEST_CHANGES.
- GLM: pending direct-API source approval, not yet sent.

## Resolved Findings

### R1 - Guard weight used in math contract but absent from TOML schema

Resolution:

- Added `guard_weight` to `contracts/toml-schema.md` aggregation schema.
- Added validation/test task T082/T085 for guard-weight behavior.

### R2 - Coarser-grid policy selector absent from TOML schema

Resolution:

- Added `coarser_grid_horizon_policy` to `contracts/toml-schema.md`.
- Added allowed values: `base`, `coarse`, `min_base_coarse`.
- Added RED/GREEN tasks T062-T066.

### R3 - Horizon role bindings and floor multiplier absent from TOML schema

Resolution:

- Added `primary_horizon_name`, `floor_horizon_name`, `short_horizon_name`, `medium_horizon_name`, `long_horizon_name`, and `floor_multiplier` to `contracts/toml-schema.md`.
- Updated `contracts/math-estimator.md` to use those role bindings.
- Added RED/GREEN tasks T050-T056.

### R4 - EWMA forecast state lifecycle unspecified

Resolution:

- Added forecast policy section to `contracts/toml-schema.md`.
- Added runtime ordering and forecast advancement rules to `contracts/runtime-api.md`.
- Updated `contracts/math-estimator.md` to define per-surface forecast state, refresh-only advancement, cold start, restart behavior, and config-fingerprint reset.
- Added RED/GREEN tasks T091-T098.

### R5 - Unknown-source cardinality cap lacked explicit RED test

Resolution:

- Evidence and runtime contracts already required bounded unknown-source diagnostics.
- Added explicit RED task T102 and GREEN task T107.

### R6 - Fewer-than-two returns jump separation lacked explicit test

Resolution:

- `contracts/toml-schema.md` and `contracts/math-estimator.md` now state fewer than two returns makes jump separation diagnostic-only.
- Added RED task T071.

### R7 - Runtime wiring and concurrency were underspecified

Resolution:

- Added `Threading and Ordering` section to `contracts/runtime-api.md`.
- Added RED tasks T025-T026 and GREEN runtime tasks T027-T033.

### R8 - Production multi-venue rule lacked programmatic enforcement test

Resolution:

- Added `single_source_explanation` to surface TOML schema.
- Defined availability as enabled public market-data client plus valid instrument mapping.
- Added RED task T035 and GREEN task T044.

### R9 - Coverage ratio, MAD scaling, and subsample coverage were underspecified

Resolution:

- Updated `contracts/math-estimator.md` with exact coverage denominator, offset-grid coverage semantics, finite-sample bipower correction, and raw MAD scaling statement.
- Added RED tasks T052, T061, and T081.

### R10 - Evidence schema version target was unnamed

Resolution:

- Updated `contracts/evidence-schema.md` to name v8 baseline and v9 target.
- Added RED/GREEN tasks T100-T106.

### R11 - External-review gate was ordered after implementation tasks

Resolution:

- Rewrote `tasks.md` so external review is Phase 2 and implementation tasks cannot execute before T006-T013 are complete.

## Re-Review Request

Reviewers should re-check only whether the above fixes close the round-1 findings and whether the planning artifacts still satisfy the three root goals:

1. Global RV runtime outside taker.
2. Multi-venue available-source routing.
3. Mathematically robust and auditable RV estimator.
