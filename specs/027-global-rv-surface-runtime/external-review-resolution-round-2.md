# External Review Resolution Log - Round 2

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

- `contracts/toml-schema.md` now requires source instruments to resolve to the same canonical base asset as the parent surface.
- `tasks.md` T034 now names the surface/source asset mismatch validation test.
- `tasks.md` T112 now names trim-equivalent duplicate surface ID tests.

### R25 - Bounded diagnostics sustained churn was implicit

Resolution:

- `tasks.md` T110 now names sustained unknown-source churn boundedness in addition to single eviction reporting.
