# External Review Resolution Log - Round 2

> Historical note: this log records design-review fixes from the original broad plan. PR #615 later narrowed the implemented math slice to Option A microstructure-noise robustness with jump diagnostics and robust aggregation. Multi-horizon and forecast references below are retained as future-scope planning history, not current PR #615 acceptance criteria.

## Review Round 2

Reviewed focused artifacts after round-1 fixes:

- `contracts/toml-schema.md`
- `contracts/math-estimator.md`
- `contracts/runtime-api.md`
- `contracts/evidence-schema.md`
- `tasks.md`
- `external-review-resolution.md`

Models:

- Claude: APPROVE with non-blocking hardening suggestions.
- Gemini: APPROVE with no findings.
- Grok: APPROVE with non-blocking hardening suggestions.
- GLM: pending direct-API source approval, not yet sent.

## Resolved Follow-Up Items

### R12 - HAR-lite weight constraints unclear

Resolution:

- `contracts/toml-schema.md` now requires HAR-lite weights to be non-negative finite with positive sum.
- `tasks.md` T102 includes HAR weight validation.

### R13 - Unsupported `mark` pair needs explicit RED test

Resolution:

- `tasks.md` T036 requires `unsupported_mark_source_class_sample_kind_is_rejected_until_runtime_routing_exists`.

### R14 - Subsample offset collisions unspecified

Resolution:

- `contracts/toml-schema.md` now adds `allow_subsample_offset_collisions`.
- Default behavior rejects `subsamples` greater than the smallest selected horizon `sampling_interval_ms`.
- If collisions are allowed, evidence must report attempted and distinct offset counts.
- `tasks.md` T063 covers the validation/semantics.

### R15 - Evidence version task must read current main

Resolution:

- `tasks.md` T109 now requires reading the current evidence version on `main`, rejecting the previous version, and bumping by exactly one.
- `tasks.md` T113 implements the exact current-main-plus-one bump.

### R16 - Forecast reset/isolation/negative-advance tests missing

Resolution:

- `tasks.md` T095 requires EWMA not to advance on observation-only updates.
- `tasks.md` T097 requires forecast state reset when config fingerprint changes.
- `tasks.md` T098 requires per-surface forecast state independence.

### R17 - Final consumed RV component needs a direct invariant test

Resolution:

- `tasks.md` T101 requires `final_ready_realized_vol_equals_configured_pricing_component`.

### R18 - HAR-lite readiness blocking needs direct test

Resolution:

- `tasks.md` T100 requires `har_lite_blocks_when_any_role_horizon_is_not_ready`.

### R19 - Measured/continuous/jump consistency needs direct test

Resolution:

- `tasks.md` T074 requires `measured_variance_equals_continuous_variance_plus_jump_variance_before_sqrt`.

## Remaining Gate

GLM direct API review still requires explicit source-send approval. No source has been sent to GLM. If approval is denied, T010 permits recording that denial and skipping GLM under the explicit source-approval rule.

## Review Round 3 Hardening

Claude final focused review approved the round-2 fixes but raised additional hardening gaps. These are treated as substantive findings under the repo review bar and resolved before implementation.

### R20 - HAR-lite negative intercept / negative output ambiguity

Resolution:

- `contracts/toml-schema.md` now requires `har_intercept` to be non-negative finite.
- `contracts/math-estimator.md` now requires HAR-lite betas to be non-negative finite with positive sum, the intercept to be non-negative finite, and forecast output to pass the valid-RV constructor before publication.
- `tasks.md` T102 now includes `har_intercept_negative_or_non_finite_rejected`.

### R21 - EWMA alpha and timestamp validation too implicit

Resolution:

- `tasks.md` T102 now names `ewma_alpha_zero_or_above_one_rejected`.
- `contracts/runtime-api.md` now requires non-monotonic forecast timestamps to leave forecast state unchanged and report the deterministic no-advance reason.
- `tasks.md` T095 now names `ewma_forecast_does_not_advance_on_non_monotonic_now_ms`.

### R22 - Production single-source explanation lacked explicit bounds

Resolution:

- `contracts/toml-schema.md` now requires single-source explanations to be trimmed, non-empty, and no longer than 512 characters.
- `tasks.md` T035 now names tests for required, trimmed, non-empty, and bounded single-source explanation behavior.

### R23 - Non-taker consumer task allowed scope drift

Resolution:

- `tasks.md` T032 now pins the required non-taker consumer to evidence/monitoring snapshot export, with maker consumption as an additive path if available on main.

### R24 - Asset binding and ID hygiene tests were implicit

Resolution:

- `contracts/toml-schema.md` now requires each source's declared canonical base and quote assets to match the parent surface while keeping `instrument_id` venue-native.
- `tasks.md` T034 now names the surface/source asset mismatch validation test.
- `tasks.md` T112 now names trim-equivalent duplicate surface ID tests.

### R25 - Bounded diagnostics sustained churn was implicit

Resolution:

- `tasks.md` T110 now names sustained unknown-source churn boundedness in addition to single eviction reporting.

## Review Round 4 Hardening

Claude and Grok approved the round-3 artifacts but identified additional clarity and coverage hardening items. They are resolved before implementation.

### R26 - Quantile and coarser-grid edge semantics

Resolution:

- `contracts/math-estimator.md` now defines nearest-rank upper quantile as `index = ceil(q * n) - 1` on the zero-indexed sorted list.
- `contracts/math-estimator.md` now states subsample integer offsets must be distinct unless TOML explicitly enables offset-collision reporting.
- `tasks.md` T064 now names both `min_base_coarse` directionality tests.
- `tasks.md` T087 now includes equal-contributor aggregation boundaries.

### R27 - HAR-lite weight upper bound and forecast invalid propagation

Resolution:

- `contracts/toml-schema.md` and `contracts/math-estimator.md` now require HAR-lite weight sum in `(0, 1]`.
- `tasks.md` T102 now requires validation for that bounded HAR weight sum.
- `tasks.md` T101 now requires invalid selected forecast components to block deterministically.

### R28 - Refresh monotonicity precedence and reset warm path

Resolution:

- `contracts/runtime-api.md` now says surface-level refresh monotonicity is checked before any forecast-state update.
- `tasks.md` T025 now covers equal `as_of_ms` refresh behavior.
- `tasks.md` T097 now includes `forecast_warm_starts_on_next_refresh_after_fingerprint_reset`.

### R29 - Noise-robust pricing component ambiguity

Resolution:

- `contracts/toml-schema.md` now requires `pricing_component = "noise_robust"` to use `noise_robust_method != "none"`.
- `tasks.md` T102 now names `pricing_component_noise_robust_requires_noise_method_not_none`.

### R30 - Unsupported mark enum guard

Resolution:

- `contracts/toml-schema.md` now requires both validation rejection and runtime construction failure if validation is bypassed for mark sources.
- `tasks.md` T036 now names both the config and runtime-construction rejection tests.

### R31 - Multi-venue degraded-source audit path

Resolution:

- `tasks.md` T038 now names `multi_venue_partial_source_down_remains_auditable_while_quorum_policy_decides_readiness`.

### R32 - Schema example single-source explanation clarity

Resolution:

- `contracts/toml-schema.md` now uses `<REQUIRED_WHEN_SINGLE_SOURCE>` in the surface example instead of an empty string.

## Review Round 5 Hardening

Claude, Gemini, Grok, and GLM approved round 4. Remaining non-blocking concerns were converted into explicit contracts or named RED tests before implementation. This file keeps its original name to preserve review-chain links, but it intentionally aggregates follow-up rounds after round 2.

### R33 - Subsample, quantile, and zero-contributor edge definitions

Resolution:

- `contracts/math-estimator.md` now defines subsample offsets over `j in 0..k`.
- `contracts/math-estimator.md` now states quantile selection is undefined for zero contributors and the engine must block with `QuorumNotReady` before aggregation.
- `tasks.md` T062 now names distinct-offset verification.
- `tasks.md` T087 now names zero-eligible-contributor blocking before aggregation.

### R34 - All-zero dispersion and MAD semantics

Resolution:

- `contracts/math-estimator.md` now states aggregate zero with all-zero contributors has zero dispersion and does not block.
- `contracts/math-estimator.md` clarifies MAD notation over eligible contributors.
- `tasks.md` T083 now names all-zero contributor dispersion/MAD behavior.

### R35 - EWMA unchanged-value and initial/equal timestamp behavior

Resolution:

- `contracts/math-estimator.md` and `contracts/runtime-api.md` now say EWMA advances on a refresh-published ready component even when the numeric value is unchanged.
- `tasks.md` T094 now names the unchanged-component refresh behavior.
- `tasks.md` T025 now names initial/equal timestamp boundary behavior.

### R36 - Deterministic runtime ordering and subscription route collisions

Resolution:

- `contracts/runtime-api.md` now requires runtime-wide refresh to visit surfaces in sorted `surface_id` order.
- `tasks.md` T027 includes sorted surface refresh order in implementation scope.
- `tasks.md` T037 now names deterministic subscription-key order for equivalent routes.

### R37 - Remaining robust-estimator boundary tests

Resolution:

- `tasks.md` T082 now names trim-removes-all-contributors rejection.
- `tasks.md` T099 now names all-zero HAR-lite valid-zero forecast behavior.

### R38 - Maker additive path test naming

Resolution:

- `tasks.md` T032 now names `maker_consumes_runtime_rv_snapshot_when_present` if maker integration exists on main.

## Review Round 6 Hardening

Claude approved round 5 and left documentation/test-polish concerns. These are resolved before implementation to avoid carrying known ambiguity into TDD.

### R39 - Subsample evidence reproducibility

Resolution:

- `contracts/math-estimator.md` now requires evidence to include actual offset timestamps used by subsampled grids.
- `tasks.md` T062 now names `subsampled_evidence_records_actual_offsets_used`.

### R40 - Jump variance identity and bipower upper-bound case

Resolution:

- `contracts/math-estimator.md` now states the measured/continuous/jump identity is enforced on the pre-annualized variance scale before square-root conversion.
- `tasks.md` T074 now names `bipower_variance_above_measured_variance_produces_zero_jump_component`.

### R41 - MAD zero-median subcases

Resolution:

- `contracts/math-estimator.md` now states median zero with all deviations zero has MAD ratio zero and does not block.

### R42 - EWMA cold-start timestamp ordering

Resolution:

- `contracts/math-estimator.md` now states cold start sets `previous_forecast_update_ms` to the refresh timestamp and later refreshes use strict `now_ms > previous_forecast_update_ms`.
- `tasks.md` T096 now names `ewma_cold_start_sets_previous_update_timestamp_for_next_refresh`.

### R43 - Pricing-component enum drift

Resolution:

- `contracts/math-estimator.md` now points final pricing selection back to the enum defined in `toml-schema.md`.

### R44 - Unknown-source eviction policy proof

Resolution:

- `tasks.md` T110 now names `unknown_source_eviction_policy_is_deterministic_and_documented`.
