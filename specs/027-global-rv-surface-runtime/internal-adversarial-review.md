# Internal Adversarial Review

## Scope Reviewed

Artifacts reviewed on branch `codex/027-global-rv-surface-runtime`:

- `spec.md`
- `plan.md`
- `research.md`
- `data-model.md`
- `tasks.md`
- `contracts/runtime-api.md`
- `contracts/toml-schema.md`
- `contracts/evidence-schema.md`
- `contracts/math-estimator.md`
- `quickstart.md`

## Verdict

Proceed to external adversarial review. No open blocking findings in the planning artifacts.

## Findings Checked

### A1 - Plan could still permit a per-strategy engine instance

Risk: The previous PR moved policy into a shared module but left each taker strategy holding its own engine instance. A shallow follow-up could repeat that pattern.

Assessment: Resolved in planning artifacts.

Evidence:

- `runtime-api.md` requires `RealizedVolSurfaceRuntime` to own every surface state instance at process level.
- `tasks.md` T006, T012-T020 require red tests and implementation that remove strategy-owned engine construction.
- `tasks.md` T021 extends consumption to maker or nearest maker-side consumer to prove the runtime is usable outside taker.

### A2 - Multi-venue wording could be vague

Risk: "Use whatever is available" could become one hardcoded source again if availability is not tied to config validation and runtime routes.

Assessment: Resolved in planning artifacts.

Evidence:

- `toml-schema.md` defines source entries by `client_id`, `instrument_id`, `source_class`, and `sample_kind` under root-owned surfaces.
- `toml-schema.md` adds a production multi-venue rule: shipped production surfaces configure every available public venue/source with a valid client/instrument mapping.
- `tasks.md` T023-T034 require validation, deduplicated subscriptions, fan-out, and production config updates.

### A3 - Mathematical robustness could remain hand-wavy

Risk: The plan names robust estimators but does not define reproducible formulas.

Assessment: Resolved by `contracts/math-estimator.md`.

Evidence:

- Fixed-grid RV, multi-horizon blending, coarser-grid/subsampled noise robustness, bipower jump separation, median/trimmed/upper-quantile aggregation, MAD blocking, EWMA, and HAR-lite formulas are specified.
- Evidence contract requires all intermediate values needed to reproduce the final pricing RV.

### A4 - Robustness could hide jumps or suppress real information

Risk: Jump-robust estimation can underprice binary tails if jumps are deleted.

Assessment: Resolved in planning artifacts.

Evidence:

- `research.md` chooses jump separation, not jump deletion.
- `math-estimator.md` emits measured, continuous, and jump components.
- `evidence-schema.md` requires the final pricing component to be named explicitly.
- `tasks.md` T053-T060 require tests for jump component serialization and non-erasure.

### A5 - Unknown source diagnostics could grow unbounded

Risk: Unknown source IDs are counted in a map, and a future raw ingestion path could create memory growth.

Assessment: Resolved at contract level; implementation must enforce it.

Evidence:

- `runtime-api.md` requires bounded diagnostics for unknown source IDs.
- `evidence-schema.md` requires bounded unknown-source counters with deterministic eviction metadata.
- `tasks.md` T069 requires unknown-source diagnostics preservation; implementation should add an explicit focused test under that task if the initial failing tests do not cover the cardinality cap.

### A6 - Forecast model could introduce opaque model risk

Risk: Forecast RV can become a hidden model rather than an auditable deterministic transform.

Assessment: Resolved in planning artifacts.

Evidence:

- `research.md` rejects GARCH/ML as first step.
- `math-estimator.md` restricts forecast modes to `none`, `ewma`, and `har_lite` with TOML-owned coefficients.
- `tasks.md` T071-T079 require deterministic forecast tests and evidence.

### A7 - Scope is large enough to violate minimal-slice discipline

Risk: Global runtime, multi-venue, and robust math are individually large changes.

Assessment: Accepted consciously because the owner explicitly rejected narrowing after prior shallow patches.

Evidence:

- `plan.md` constitution check documents a full-scope exception.
- `tasks.md` states that global runtime, multi-venue production wiring, and robust estimator work must not be split unless the owner changes scope in writing.

## Required External Review Focus

Ask Claude, Gemini, Grok, and GLM to challenge these exact questions:

1. Does this design truly make RV globally usable outside the taker, or can implementation still hide per-strategy ownership?
2. Does the multi-venue plan guarantee all configured available sources are used without Rust hardcodes?
3. Are the robust estimator formulas precise, auditable, and suitable for binary oracle pricing?
4. Does jump separation preserve tail information rather than suppress it?
5. Are the tasks ordered so TDD catches the root problems before implementation?

## Internal Recommendation

Run external adversarial reviews before implementation. If any reviewer finds a substantive issue, update the spec/plan/tasks/contracts first, then re-review. Do not implement from this branch until the plan has unanimous approval or a model is explicitly skipped after more than two consecutive relay failures.

## Current-State Follow-Up Audit - 2026-06-09

Scope reviewed after implementation and post-fix approval comments:

- `specs/027-global-rv-surface-runtime/contracts/evidence-schema.md`
- `specs/027-global-rv-surface-runtime/contracts/math-estimator.md`
- `specs/027-global-rv-surface-runtime/contracts/runtime-api.md`
- `specs/027-global-rv-surface-runtime/tasks.md`
- `tests/bolt_v3_realized_volatility.rs`
- `tests/bolt_v3_realized_volatility_runtime.rs`
- `.specify/feature.json`
- `AGENTS.md`

### F1 - Active Speckit pointer cannot be moved to feature 027

Risk: The `speckit-plan` workflow says the active agent context should point at the generated plan, but this repository's current source-fence gate requires `.specify/feature.json` and `AGENTS.md` to keep pointing at `specs/023-nt-order-intent-layer/plan.md`.

Disposition: Resolved by preserving the CI-required active pointer and keeping feature 027 artifacts under `specs/027-global-rv-surface-runtime/`.

Evidence:

- `just source-fence` fails when the active pointer is changed to feature 027.
- `just source-fence` passes again after restoring the feature 023 pointer.

### F2 - Contract docs drifted from Rust evidence/runtime names

Risk: External consumers could implement against fields that the Rust structs do not emit.

Disposition: Resolved.

Evidence:

- Removed non-emitted source diagnostic timestamp fields from `contracts/evidence-schema.md`.
- Changed runtime snapshot contract from `aggregation_method` to `aggregation`.

### F3 - Math contract overstated coarser/subsampled/jump identities

Risk: Reviewers and future implementers could believe unsupported policies or identities are guaranteed.

Disposition: Resolved.

Evidence:

- `coarser_grid_policy` now names `coarse_only` and `min_base_coarse`.
- Annualization now names elapsed grid-return seconds rather than full-window seconds.
- Subsampled RV now documents base coverage first, then ready offset lanes with at least two grid prices.
- Jump identity is explicitly per source; surface-level component fields are independently aggregated marginals.

### F4 - Residual test gaps from approval comment

Risk: Previous PR comment listed coverage gaps while still approving.

Disposition: Partially resolved with direct focused tests.

Evidence:

- Added `coarser_grid_policy_selects_coarse_only_component`.
- Added `coarser_grid_min_base_coarse_uses_lower_component`.
- Added `runtime_refresh_ignores_stale_and_equal_explicit_refresh_timestamps`.
- Added `runtime_direct_observe_wrong_sample_kind_rejects_without_republishing_snapshot`.
- Focused checks pass: `cargo test --locked --test bolt_v3_realized_volatility`, `cargo test --locked --test bolt_v3_realized_volatility_runtime`, `cargo fmt --check`, `git diff --check`, and `just source-fence`.

### Current Verdict

Proceed to external re-review on the updated diff before claiming goal completion. The remaining approval requirement is current-source adversarial review/approval from Claude, Gemini, Grok, and GLM, or an explicit failure-rule skip where allowed by the task contract.
