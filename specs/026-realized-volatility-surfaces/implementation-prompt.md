# Realized Volatility Surfaces Implementation Prompt

> **Historical implementation prompt — do not reuse it.** Current `main`,
> `AGENTS.md`, and the exact-head review request are authoritative.

Use this prompt to start implementation of the realized-volatility surface
feature. It captures the current reviewed design state and late review findings.

## Objective

Implement the RV-specific, TOML-owned, multi-source realized-volatility surface
defined in:

- `specs/026-realized-volatility-surfaces/spec.md`
- `specs/026-realized-volatility-surfaces/plan.md`

The implementation must produce an audit-grade `RealizedVolSnapshot` that taker
pricing and future strategies can consume without owning RV source selection,
sampling, quorum, dispersion, or readiness policy.

## Review Status

- Gemini custom review approved the revised full spec/plan packet after the
  zero-aggregate dispersion blocker was fixed.
  - Approved job: `7fd73204-3215-4586-b0c9-d8d77844ca3d`
  - Reviewed files:
    - `specs/026-realized-volatility-surfaces/spec.md`
    - `specs/026-realized-volatility-surfaces/plan.md`
- Grok custom review approved the same revised full spec/plan packet.
  - Approved job: `job_83229dc8-a48d-4ee3-8494-fd89fb20b565`
  - Reviewed files:
    - `specs/026-realized-volatility-surfaces/spec.md`
    - `specs/026-realized-volatility-surfaces/plan.md`
- Claude did not produce a design rejection. Its review slot failed for reviewer
  tooling/resend reasons after source transmission, so it is not an approval.
- Kimi must not be used for this work unless the operator explicitly changes
  that instruction.

This is implementation-ready review confidence, not 100 percent certainty.
Actual Rust integration remains subject to TDD, focused tests, source fences,
formatting, clippy, and final verification.

## Hard Constraints

- Do not implement from stale branches or older conversation examples.
- Do not mention or depend on unrelated PRs.
- Do not encode asset-specific, token-specific, venue-specific, or provider-
  specific defaults in Rust, docs, tests, or examples.
- Use opaque placeholders only: `<surface_id>`, `<SOURCE_ID_A>`,
  `<SOURCE_ID_B>`, `<BASE_ASSET>`, `<QUOTE_ASSET>`, `<DATA_CLIENT_ID>`,
  `<INSTRUMENT_ID>`.
- Runtime values must come from TOML config. No runtime hardcodes.
- Keep naming RV-specific: `realized_volatility`, `RealizedVol`, or `RV`.
  Do not introduce new generic volatility or implied-volatility surfaces.
- Strategies produce intent and strategy-local signal state only. They may
  forward normalized RV observations and consume pricing output, but must not
  implement RV policy.
- Taker pricing may consume `RealizedVolSnapshot`, but must not own RV sampling,
  quorum, dispersion, source readiness, or source selection.
- Taker pricing must fail closed on a missing, stale, mismatched, or not-ready
  snapshot. It must not fall back to a strategy-owned internal RV estimator.

## Engine Invariants

- Engine state is keyed by opaque `source_id` only.
- Provider, venue, client, and instrument identifiers are provenance fields only.
- Unknown source ids are counted in snapshot-level unknown-source rejection
  counters, not silently dropped.
- Per-source rejection counters and the last rejection reason must be auditable.
- Same-event handling:
  - exact duplicate `(event_ts_ms, recv_ts_ms)` is rejected
  - same `event_ts_ms` with larger `recv_ts_ms` replaces the stored sample
  - same `event_ts_ms` with equal or lower `recv_ts_ms` must not overwrite
  - replacement must not create a realized-return interval by itself
- Fixed-grid sampling is required. Event update frequency must not determine RV
  weight.
- LOCF may use the latest valid pre-window observation for the first grid cell
  when it is still within `max_source_age_ms`.
- Missing grid cells reduce coverage and never create synthetic returns.
- Inter-sample gap is measured over valid grid timestamps.
- Flat, sufficiently covered valid prices produce `Some(0.0)` RV and can be
  ready.
- RV is computed per source before aggregation. Never compute RV from a
  source-switching composite price.
- Initial aggregation is nearest-rank upper quantile over ready per-source RVs.
- `upper_quantile` must be in `[0.5, 1.0]`.
- Cross-source dispersion is:
  `(max_ready_source_rv - min_ready_source_rv) / aggregate_rv`.
- If aggregate RV is zero and all ready source RVs are zero, dispersion is zero.
- If aggregate RV is zero and ready source RVs diverge, dispersion must block.
- Preserve high isolated returns; do not winsorize returns in the initial
  implementation.
- Source samples must be pruned to a bounded retention horizon.

## Snapshot Contract

`RealizedVolSnapshot` must include:

- `surface_id`
- `as_of_ms`
- readiness and blockers
- annualized realized-volatility decimal
- `seconds_per_annum`
- aggregation method
- sources used
- per-source diagnostics
- per-source rejection counters
- unknown-source rejection counters
- config fingerprint

The closed `RealizedVolBlockReason` set is:

- `InvalidConfig`
- `QuorumNotReady`
- `SourceStale`
- `CoverageBelowMinimum`
- `InterSampleGapExceeded`
- `SourceClassMismatch`
- `SampleKindMismatch`
- `CrossSourceDispersion`
- `AnnualizationBasisInvalid`
- `NotWarm`

Expose an exhaustive `ALL` constant and test it against this list.

The config fingerprint must be a stable SHA-256 hash over canonical serialized
realized-volatility engine config. Mutating policy must change the fingerprint.

## TDD Protocol

Follow the plan task by task. Do not write production code until the matching
test has been written and observed failing for the expected reason.

Start with Task 1:

1. Add `tests/bolt_v3_realized_volatility.rs` with the engine tests from the
   reviewed plan.
2. Run `cargo test --test bolt_v3_realized_volatility -- --nocapture` and
   confirm the expected unresolved import failure.
3. Implement the smallest RV module and `src/lib.rs` export needed to pass.
4. Run the focused test again and iterate only on the tested behavior.

Then continue through the reviewed plan:

- root TOML surface config and validation
- surfaced-mode rejection of legacy RV paths
- taker pricing snapshot consumption with no fallback
- strategy observation forwarding only
- RV evidence fields
- source-fence enforcement

## Required Verification Before Claiming Completion

Run the final verification commands listed in `plan.md`, including:

- `cargo fmt --check`
- `cargo clippy --locked --lib -- -D warnings`
- focused RV, pricing, config, evidence, and source-fence tests
- `cargo test --lib -- --nocapture`
- `git diff --check`
- repo source fences as applicable

If any command is not run or does not pass, report that directly.
