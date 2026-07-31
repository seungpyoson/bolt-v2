# Implementation Plan: Global RV Surface Runtime

**Branch**: `027-global-rv-surface-runtime` | **Date**: 2026-06-08 | **Spec**: `specs/027-global-rv-surface-runtime/spec.md`  
**Input**: Feature specification from `specs/027-global-rv-surface-runtime/spec.md`

## Summary

Build a runtime-owned realized-volatility surface service that is usable by taker, maker, and future consumers by `surface_id`; move RV observation routing and subscriptions out of `BinaryOracleEdgeTaker`; configure multiple venue/source bindings for each production binary-oracle RV surface where existing data clients support the asset; and upgrade RV math from single-window fixed-grid RV to Option A microstructure-noise robust, jump-aware, robustly aggregated RV.

This is intentionally not a small compatibility patch. The current post-PR #609 state still has per-strategy RV engine ownership. This plan replaces that with a global surface runtime and keeps strategy modules as consumers only. Multi-horizon blending and forecast-oriented RV remain future slices, not PR #615 acceptance criteria.

## Technical Context

**Language/Version**: Rust, existing repo toolchain  
**Primary Dependencies**: Existing crate modules, NautilusTrader Rust APIs, TOML config parsing, serde evidence serialization  
**Storage**: In-memory runtime state; TOML config; evidence serialization output  
**Testing**: TDD with focused Rust tests, source-fence tests, schema/evidence tests, CI nextest shards  
**Target Platform**: Pure Rust live trading binary using NT Rust APIs  
**Project Type**: Single Rust application/library crate  
**Performance Goals**: One physical market-data subscription per unique data-client/instrument/source-kind stream; snapshot lookup by `surface_id` is O(log n) or better; per-snapshot computation bounded by configured source/sample windows
**Constraints**: TOML-owned runtime values only; no strategy-owned RV lifecycle; no hardcoded asset/venue defaults in Rust; no duplicate RV paths; fail-closed live trading behavior; no credentials printed  
**Scale/Scope**: Production binary-oracle surfaces for BTC, ETH, SOL, BNB, XRP, DOGE plus extensible runtime for future taker/maker consumers and additional surfaces

## Constitution Check

- **I. NT-First Thin Layer**: PASS. The runtime coordinates NT market-data subscriptions and does not rebuild adapter behavior, cache truth, order lifecycle, or venue simulation.
- **II. Generic Core, Concrete Edges**: PASS. Core RV runtime remains keyed by opaque `surface_id` and `source_id`; concrete clients/instruments stay in TOML/registry bindings.
- **III. Single Path And Config-Controlled Runtime**: PASS REQUIRED. The implementation must delete per-strategy RV engine ownership and leave one global runtime path. Any fallback estimator is a gate failure.
- **IV. Test-First Safety Gates**: PASS REQUIRED. Tasks require red-before-green tests for every behavior change.
- **V. Evidence Before Claims**: PASS REQUIRED. External reviews happen only after branch artifacts are committed/pushed and exact-head CI is green.
- **VI. Minimal Slice Discipline**: PASS. The accepted slice is global RV runtime plus multi-venue plus Option A noise-robust RV with jump diagnostics and robust aggregation. Multi-horizon and forecast work is tracked as future scope and is not claimed by PR #615.
- **VII. Research And Analytics Stay NT-First**: PASS. No research notebooks or alternate runtime path are introduced.

## Project Structure

### Documentation (this feature)

```text
specs/027-global-rv-surface-runtime/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── runtime-api.md
│   ├── toml-schema.md
│   └── evidence-schema.md
└── tasks.md
```

### Source Code (repository root)

```text
src/
├── bolt_v3_realized_volatility.rs          # RV math, estimates, snapshots, source evaluation
├── bolt_v3_realized_volatility_runtime.rs  # runtime-level surface service and subscription routing
├── bolt_v3_config.rs                       # TOML schema extensions
├── bolt_v3_validate.rs                     # root validation for sources, horizons, methods, clients
├── bolt_v3_current_evidence/               # current evidence schema/version updates
├── bolt_v3_taker_pricing.rs                # snapshot consumer only
├── strategies/
│   ├── registry.rs                         # build context exposes global snapshot/runtime handle
│   └── binary_oracle_edge_taker/           # no RV engine ownership; consumes snapshots only
└── lib.rs                                  # export runtime module

tests/
├── bolt_v3_realized_volatility.rs
├── bolt_v3_realized_volatility_runtime.rs
├── bolt_v3_realized_volatility_source_fence.rs
├── bolt_v3_current_evidence_runtime.rs
├── bolt_v3_strategy_registration.rs
└── bolt_v3_taker_pricing.rs

config/
├── root.toml
└── strategies/binary_oracle_*.toml
```

**Structure Decision**: Add a dedicated runtime-level RV service module rather than hiding lifecycle inside strategy code or expanding the existing engine file. Keep math/state primitives in `bolt_v3_realized_volatility.rs`; keep runtime ownership, subscription dedupe, and snapshot publication in `bolt_v3_realized_volatility_runtime.rs`.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| Broad single branch for global runtime + multi-venue + Option A robust math | User required these three root problems to move together and approved Option A as the first math step | A smaller branch would preserve either per-strategy RV ownership, single-source production RV, or the noisy fixed-grid estimator |
| New runtime module | Needed to remove RV lifecycle from strategies and support shared consumers | Keeping engine instances in each strategy repeats PR #609's root failure |
| Expanded estimator schema | Needed for noise-aware, jump-aware, robust aggregation policies and reserved future horizon/forecast compatibility | Only adding more venues still leaves mathematically brittle single-window RV |

## Phase 0: Research Decisions

See `research.md` for detailed decisions. Summary:

- Runtime ownership moves to a global RV surface runtime, not strategy structs.
- Multi-venue starts from existing configured public data clients and only enables sources with validated client/instrument support.
- First math upgrade is Option A microstructure-noise robustness using auditable coarser-grid or deterministic offset-grid subsampled RV before realized-kernel complexity.
- Jump handling separates continuous RV and jump component rather than deleting jumps.
- Cross-source aggregation extends current upper-quantile/dispersion with median, trimmed mean, and median/upper-quantile guard policies.
- Multi-horizon and forecast layers are future slices; any later forecast support must keep measured-vs-forecast evidence separate.

## Phase 1: Design And Contracts

Design artifacts:

- `data-model.md`: runtime, surface state, binding, subscription key, robust estimates, snapshots, and future horizon/forecast placeholders.
- `contracts/runtime-api.md`: runtime API expected by consumers and strategy/build context.
- `contracts/toml-schema.md`: TOML schema extensions for runtime, sources, Option A estimators, aggregation, and future horizon/forecast placeholders.
- `contracts/evidence-schema.md`: evidence fields and schema-version requirements.
- `quickstart.md`: verification scenarios and CI gates.

## Implementation Rules

- No production code before a failing test.
- Do not move RV policy into `src/strategies/*`.
- Do not add asset/venue defaults in Rust. All concrete production sources are TOML.
- Do not leave old and new RV lifecycle paths active at the same time.
- Do not request external relay review until branch has plan/tasks committed and internal adversarial review issues are resolved.
- Do not begin implementation until Claude, Gemini, Grok, and GLM adversarial reviews unanimously approve plan/tasks.

## Post-Design Constitution Check

- Generic core remains TOML keyed by `surface_id` and `source_id`.
- Strategy code is consumer-only and source-fenced.
- Runtime values remain TOML-owned.
- TDD is explicitly required in tasks.
- Full-scope branch is justified by explicit user direction; scope remains bounded to RV surface runtime/math.
