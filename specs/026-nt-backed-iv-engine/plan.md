# Implementation Plan: NT-Backed IV Engine

**Branch**: `026-nt-backed-iv-engine` | **Date**: 2026-06-07 | **Spec**: `specs/026-nt-backed-iv-engine/spec.md`
**Input**: Feature specification from `specs/026-nt-backed-iv-engine/spec.md`

## Summary

Build a live-integrated IV engine that uses every IV/options capability exposed by the NautilusTrader Rust APIs pinned in `Cargo.toml`: option greeks, option chains, aggregate greeks, adapter custom volatility data, raw payload preservation, indexed IV points, smiles, surfaces, source health, explicit interpolation/fallback/quorum policies, and derived IV through NT math helpers. Root TOML loads IV profiles, live-node startup owns NT IV subscriptions, and strategies consume an IV query handle through one generic API. This plan is IV-only; FV and RV are not prerequisites and are not implemented here.

## Technical Context

**Language/Version**: Rust, edition per workspace.
**Primary Dependencies**: NautilusTrader Rust crates pinned by `Cargo.toml`, `serde`, `toml`, existing Bolt config, live-node, strategy-registration, test, and source-fence tooling.
**Storage**: In-memory live IV store with TOML-owned retention bounds; no durable storage in this feature.
**Testing**: `cargo test`, `cargo fmt --check`, `cargo clippy --locked --lib -- -D warnings`, `just source-fence`.
**Target Platform**: Existing pure Rust Bolt live binary and strategy runtime.
**Project Type**: Shared Rust crate module plus root config, live-node startup, strategy-registration/query-handle integration, tests, docs, and source-fence coverage.
**Performance Goals**: Store and policy operations are bounded by configured profile, source, series, strike, retention, interpolation, fallback, and quorum limits; no unbounded subscription or retention growth.
**Constraints**: No hardcoded strategy, venue, market, asset, cadence, instrument ID, source ID, timeout, quantity, or policy value. Runtime behavior comes from TOML. No Python layer. No FV/RV dependency.
**Scale/Scope**: All configured NT IV/options sources for all configured IV profiles, with isolation by profile ID, source ID, and strategy selector authorization.

## Constitution Check

*GATE: Must pass before implementation. Re-check after design.*

- **NT-First Thin Layer**: PASS. NT owns data adapters, subscriptions, greeks structures, option-chain structures, msgbus topics, and IV/greeks math helpers. Bolt owns typed TOML config, source planning, retention, fail-closed query policy, and strategy-facing API.
- **Generic Core, Concrete Edges**: PASS. The IV engine core is strategy-, venue-, market-, asset-, and cadence-agnostic. Concrete data clients and selectors are TOML-owned.
- **Single Path And Config-Controlled Runtime**: PASS. One IV engine owns live IV subscriptions, NT helper-backed IV derivation, and strategy IV access. Strategies do not create parallel NT IV subscriptions or strategy-local IV derivation paths.
- **Group By Change**: PASS. One IV profile owns source lifecycle, source policies, strategy authorization, enabled products, and query policies.
- **Test-First Safety Gates**: PASS. Each workstream starts with failing tests and source-fence checks.
- **Evidence Before Claims**: PASS. NT capability scope is proven by an inventory ledger generated from the Cargo-pinned checkout.
- **Minimal Slice Discipline**: PASS with explicit scope. This feature is broad within IV because the user requested all NT IV/options capabilities; it still excludes FV/RV and submit/admission behavior.
- **Research And Analytics Stay NT-First**: PASS. Derived IV uses NT helpers; raw NT data is preserved.

## Project Structure

### Documentation (this feature)

```text
specs/026-nt-backed-iv-engine/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── checklists/
│   └── requirements.md
└── contracts/
    └── iv-engine-api.md
```

### Source Code (repository root)

```text
src/bolt_v3_iv/
├── mod.rs                 # public IV engine module exports
├── capability.rs          # Cargo-pinned NT IV/options capability ledger
├── config.rs              # TOML-owned typed IV profile config
├── subscription.rs        # NT subscribe/unsubscribe planning
├── ingest.rs              # raw NT/custom event ingestion
├── store.rs               # bounded raw and indexed IV store
├── query.rs               # strategy-facing IV query API
├── derive.rs              # NT helper-backed derived IV/greeks
├── policy.rs              # interpolation, fallback, quorum, projection policy
└── health.rs              # source health and reject reasons

src/lib.rs                 # exports bolt_v3_iv
src/bolt_v3_config.rs      # loads root IV profile config
src/bolt_v3_live_node.rs   # starts/stops IV engine with live runtime
src/bolt_v3_strategy_registration.rs
                          # gives authorized strategies an IV query handle

tests/
├── bolt_v3_iv_capability.rs
├── bolt_v3_iv_config.rs
├── bolt_v3_iv_live_integration.rs
├── bolt_v3_iv_subscription.rs
├── bolt_v3_iv_ingest.rs
├── bolt_v3_iv_store.rs
├── bolt_v3_iv_query.rs
├── bolt_v3_iv_policy.rs
├── bolt_v3_iv_derive.rs
└── bolt_v3_iv_source_fence.rs
```

**Structure Decision**: Use a module directory because the feature has separate capability, config, subscription, ingestion, store, query, policy, derivation, and health responsibilities. The directory is still integrated through existing root config, live-node, strategy-registration, and crate-export modules so it cannot become an isolated library.

## Blocking Findings Folded Into The Plan

| Blocking finding | Plan response |
|---|---|
| "Use all NT offers" is unbounded | Add `capability.rs` and `tests/bolt_v3_iv_capability.rs`; the ledger is generated from the Cargo-pinned NT checkout and every discovered IV/options surface must be classified. |
| Subscribing to everything can explode cardinality | Support every NT source type, but subscribe only to TOML-configured clients, instruments, series, ranges, and selectors. |
| Derived IV needs complete inputs | `derive.rs` requires price, underlying, strike, side, time-to-expiry, rate, carry, source timestamps, and convention policy. Missing/invalid inputs reject. |
| Venue/convention data must not be normalized away | Store raw NT payloads and index convention, basis, source, and provenance with every IV product. |
| Strategies need IV access but should not own mechanics | Expose raw and indexed IV products through `query.rs`; source-fence rejects strategy-local NT IV subscriptions and helper-backed IV derivation globally in strategy modules. |
| Custom volatility data is separate evidence | Model custom volatility as `IvEvidence`, not `IvPoint`, unless a configured projection explicitly derives an IV point. |
| NT timestamps are nanoseconds | Introduce typed timestamp handling and tests for freshness/retention/query conversion. |
| Option-chain lifecycle can leak stale surfaces | Add subscription lifecycle, unsubscribe, stale marking, and retention eviction tests. |
| Config shape can violate group-by-change | Replace separate source and strategy allow-list sections with one `IvProfile` boundary that owns sources, strategy authorization, lifecycle, and policies. |
| Plan could produce an isolated library | Add explicit edits and tests for `src/lib.rs`, root TOML loading, live-node startup, and strategy-registration query handles. |
| Aggregate greeks were claimed but not modeled | Add `IvAggregateGreeks`, raw aggregate preservation, indexed aggregate product tests, and query API coverage. |
| Interpolation/fallback/quorum were named but not testable | Add `policy.rs`, policy entities, fail-closed query behavior, and provenance requirements. |
| Custom evidence existed only in data-model | Add `IvEvidence` to the spec entity list and API product list. |
| Source-fence was conditional | Make strategy-owned NT IV subscriptions and NT helper-backed strategy-local IV derivation globally rejected in strategy modules. |

## Workstreams

**W1 - NT capability ledger.** Build the source-backed inventory of NT IV/options surfaces at the Cargo-pinned revision. Cover model types, greeks helpers, msgbus APIs, data actor methods, data engine publish paths, option-chain manager, adapter support reachable through Rust, and custom volatility data reachable through NT custom data. Gate: unclassified NT IV/options surfaces fail tests.

**W2 - Typed IV profile config.** Add TOML schema and validation for IV profiles that own strategy authorization, source IDs, data clients, source kinds, subscription selectors, params, accepted conventions, IV bases, freshness, retention, projection, fallback, interpolation, extrapolation, quorum, and derived-input policies. Gate: invalid TOML fails closed with exact field diagnostics, and source rename fixtures prove group-by-change.

**W3 - Root config and live runtime wiring.** Export the IV module, load IV profiles from root TOML, start/stop the IV engine from live-node startup/shutdown, and pass authorized IV query handles through strategy registration. Gate: live integration tests prove configured profiles produce live subscription plans and strategies receive only authorized query handles.

**W4 - Subscription planner.** Convert typed profiles into NT subscribe/unsubscribe requests for option greeks, option chains, aggregate greeks, and custom volatility data. Gate: a test data actor records the exact source kinds requested by each TOML fixture, including reload and source removal.

**W5 - Raw ingestion and indexed products.** Ingest NT `OptionGreeks`, `OptionChainSlice`, aggregate greeks, and custom volatility events. Preserve raw payloads and build `IvPoint`, `IvGreeksPoint`, `IvAggregateGreeks`, `IvSmile`, `IvSurface`, `IvEvidence`, and `IvSourceHealth`. Gate: tests prove mark/bid/ask, greeks, convention, underlying price, open interest, timestamps, calls, puts, quotes, aggregate greeks, evidence values, and source provenance are preserved.

**W6 - Strategy-facing query API and policies.** Expose raw payload, IV point, greeks, aggregate greeks, smile, surface, evidence, source-health, scalar projection, interpolation, fallback, and quorum queries. Gate: multiple strategy harnesses use the same API with different profiles/selectors, unauthorized selectors reject, and every policy decision records provenance or rejects.

**W7 - NT helper-backed derived IV.** Implement derived IV and derived greeks through NT math helpers only when configured inputs are complete and valid. Gate: complete fixtures produce finite outputs; every missing/invalid input class produces a typed rejection.

**W8 - Lifecycle, retention, and source-fence hardening.** Add unsubscribe, reload, stale, eviction, source removal, and direct-subscription/source-helper source-fence tests. Gate: stale or removed data cannot satisfy current queries, and strategy-local IV subscription mechanics or helper-backed derivation fail source-fence.

## Phase Plan

### Phase 0: Research

Produce `research.md` with decisions for:

- Cargo-pinned NT capability inventory method.
- Supported NT IV/options source kinds.
- Strategy API exposure rule for raw and indexed data.
- IV profile config grouping and group-by-change validation.
- Root config, live-node, and strategy-registration integration points.
- Timestamp representation and conversion policy.
- Derived-IV input contract.
- Aggregate greeks indexed-product contract.
- Interpolation, fallback, extrapolation, and quorum policy contracts.
- Custom volatility evidence classification.
- Retention and source lifecycle semantics.

### Phase 1: Design Contracts

Produce:

- `data-model.md` defining all IV entities and validation rules.
- `contracts/iv-engine-api.md` defining strategy-facing API contracts and source-fence boundaries.
- `quickstart.md` showing how an operator configures one IV profile without asset, venue, market, cadence, or concrete strategy-specific examples.

### Phase 2: Implementation Tasks

Generate `tasks.md` only after this plan is approved. Tasks must be TDD and independently reviewable by workstream. No runtime code is written from this plan until tasks exist and are approved.

## Complexity Tracking

| Decision | Why needed | Simpler alternative rejected because |
|---|---|---|
| Full NT capability ledger | "All NT offers" must be testable | A hand-authored list will miss surfaces or drift after NT updates |
| Raw plus indexed store | Strategies need all NT data and generic products | Raw-only forces strategies to rebuild IV state; indexed-only loses NT data |
| Module directory | Capability, config, subscription, ingestion, store, query, derivation, and health are separate responsibilities | A single file would obscure boundaries and make review harder |
| Strategy source-fence | Strategies may consume IV but must not own mechanics | Relying on convention would recreate dual IV paths |
| Custom volatility evidence type | Broad vol data is useful but not identical to option-chain IV | Treating it as a normal IV point would mislead consumers |
| IV profile boundary | Repo rules require group-by-change | Separate source and strategy-access sections make a source swap a multi-edit operation |
| Explicit policy module | Interpolation, fallback, and quorum can change answers | Leaving policy in prose would make strategy output non-auditable |
| Live integration workstream | The deliverable must subscribe through NT at runtime | An isolated module could pass unit tests without becoming the IV engine |

## Verification

- `cargo test --locked bolt_v3_iv`
- `cargo test --locked --test bolt_v3_iv_capability`
- `cargo test --locked --test bolt_v3_iv_config`
- `cargo test --locked --test bolt_v3_iv_live_integration`
- `cargo test --locked --test bolt_v3_iv_subscription`
- `cargo test --locked --test bolt_v3_iv_ingest`
- `cargo test --locked --test bolt_v3_iv_store`
- `cargo test --locked --test bolt_v3_iv_query`
- `cargo test --locked --test bolt_v3_iv_policy`
- `cargo test --locked --test bolt_v3_iv_derive`
- `cargo test --locked --test bolt_v3_iv_source_fence`
- `cargo test --locked --test config_parsing`
- `cargo fmt --check`
- `cargo clippy --locked --lib -- -D warnings`
- `just source-fence`
