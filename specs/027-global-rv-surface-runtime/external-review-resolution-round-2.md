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
