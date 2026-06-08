# Implementation Plan: Global RV Surface Runtime

**Branch**: `027-global-rv-surface-runtime` | **Date**: 2026-06-08 | **Spec**: `specs/027-global-rv-surface-runtime/spec.md`  
**Input**: Feature specification from `specs/027-global-rv-surface-runtime/spec.md`

## Summary

Build a runtime-owned realized-volatility surface service that is usable by taker, maker, and future consumers by `surface_id`; move RV observation routing and subscriptions out of `BinaryOracleEdgeTaker`; configure multiple venue/source bindings for each production binary-oracle RV surface where existing data clients support the asset; and upgrade RV math from single-window fixed-grid RV to auditable multi-horizon, noise-aware, jump-aware, robustly aggregated RV.

This is intentionally not a small compatibility patch. The current post-PR #609 state still has per-strategy RV engine ownership. This plan replaces that with a global surface runtime and keeps strategy modules as consumers only.

## Technical Context

**Language/Version**: Rust, existing repo toolchain  
**Primary Dependencies**: Existing crate modules, NautilusTrader Rust APIs, TOML config parsing, serde evidence serialization  
**Storage**: In-memory runtime state; TOML config; evidence serialization output  
**Testing**: TDD with focused Rust tests, source-fence tests, schema/evidence tests, CI nextest shards  
**Target Platform**: Pure Rust live trading binary using NT Rust APIs  
**Project Type**: Single Rust application/library crate  
**Performance Goals**: One physical market-data subscription per unique data-client/instrument/source-kind stream; snapshot lookup by `surface_id` is O(log n) or better; per-snapshot computation bounded by configured source/horizon/sample windows  
**Constraints**: TOML-owned runtime values only; no strategy-owned RV lifecycle; no hardcoded asset/venue defaults in Rust; no duplicate RV paths; fail-closed live trading behavior; no credentials printed  
**Scale/Scope**: Production binary-oracle surfaces for BTC, ETH, SOL, BNB, XRP, DOGE plus extensible runtime for future taker/maker consumers and additional surfaces

## Constitution Check

- **I. NT-First Thin Layer**: PASS. The runtime coordinates NT market-data subscriptions and does not rebuild adapter behavior, cache truth, order lifecycle, or venue simulation.
- **II. Generic Core, Concrete Edges**: PASS. Core RV runtime remains keyed by opaque `surface_id` and `source_id`; concrete clients/instruments stay in TOML/registry bindings.
- **III. Single Path And Config-Controlled Runtime**: PASS REQUIRED. The implementation must delete per-strategy RV engine ownership and leave one global runtime path. Any fallback estimator is a gate failure.
- **IV. Test-First Safety Gates**: PASS REQUIRED. Tasks require red-before-green tests for every behavior change.
- **V. Evidence Before Claims**: PASS REQUIRED. External reviews happen only after branch artifacts are committed/pushed and exact-head CI is green.
- **VI. Minimal Slice Discipline**: CONSCIOUS FULL-SCOPE EXCEPTION. The user explicitly rejected narrowing scope. This branch intentionally covers one named broader slice: global RV runtime plus multi-venue plus robust RV math. The slice remains bounded to RV surfaces and does not touch submit/admission/execution mechanics.
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
├── bolt_v3_realized_volatility_runtime.rs  # new runtime-level surface service and subscription routing
├── bolt_v3_config.rs                       # TOML schema extensions
├── bolt_v3_validate.rs                     # root validation for sources, horizons, methods, clients
├── bolt_v3_decision_evidence.rs            # evidence schema/version updates
├── bolt_v3_taker_pricing.rs                # snapshot consumer only
├── strategies/
│   ├── registry.rs                         # build context exposes global snapshot/runtime handle
│   └── binary_oracle_edge_taker/           # remove RV engine ownership; consume snapshots only
└── lib.rs                                  # export runtime module

tests/
├── bolt_v3_realized_volatility.rs
├── bolt_v3_realized_volatility_runtime.rs
├── bolt_v3_realized_volatility_source_fence.rs
├── bolt_v3_decision_evidence.rs
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
| Broad single branch for global runtime + multi-venue + robust math | User explicitly required the full scope and stated prior narrowing failed by leaving RV inside taker | A smaller branch would preserve the wrong end state and fail the objective |
| New runtime module | Needed to remove RV lifecycle from strategies and support shared consumers | Keeping engine instances in each strategy repeats PR #609's root failure |
| Expanded estimator schema | Needed for multi-horizon, noise-aware, jump-aware, robust aggregation policies | Only adding more venues still leaves mathematically brittle single-window RV |

## Phase 0: Research Decisions

See `research.md` for detailed decisions. Summary:

- Runtime ownership moves to a global RV surface runtime, not strategy structs.
- Multi-venue starts from existing configured public data clients and only enables sources with validated client/instrument support.
- First math upgrade is multi-horizon RV with TOML-owned blend/floor/regime policy.
- Noise robustness starts with an auditable subsampled or multi-scale estimator mode before realized-kernel complexity.
- Jump handling separates continuous RV and jump component rather than deleting jumps.
- Cross-source aggregation extends current upper-quantile/dispersion with median/MAD or equivalent robust policy.
- Forecast layer is optional and measured-vs-forecast evidence must stay separate.

## Phase 1: Design And Contracts

Design artifacts:

- `data-model.md`: runtime, surface state, binding, subscription key, horizon estimates, robust estimates, snapshots.
- `contracts/runtime-api.md`: runtime API expected by consumers and strategy/build context.
- `contracts/toml-schema.md`: TOML schema extensions for runtime, sources, horizons, estimators, aggregation, forecast policy.
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
