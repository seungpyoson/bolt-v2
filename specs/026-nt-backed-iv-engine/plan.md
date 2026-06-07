# Implementation Plan: NT-Backed IV Engine

**Branch**: `026-nt-backed-iv-engine` | **Date**: 2026-06-07 | **Spec**: `specs/026-nt-backed-iv-engine/spec.md`
**Input**: Feature specification from `specs/026-nt-backed-iv-engine/spec.md`

## Summary

Build a live-integrated IV engine that uses every IV/options capability exposed by the NautilusTrader Rust APIs pinned in `Cargo.toml`: option greeks, option chains, aggregate greeks, adapter custom implied-volatility data, raw payload preservation, indexed IV points, smiles, surfaces, source health, explicit projection/interpolation/fallback/quorum policies, typed helper policy, typed provenance decisions, selector-scoped authorization, and derived IV through NT math helpers. Root TOML loads IV profiles, live-node startup owns NT IV subscriptions, and strategies consume an IV query handle through one generic API. This plan is IV-only; FV and RV are not prerequisites and are not implemented here.

## Technical Context

**Language/Version**: Rust, edition per workspace.
**Primary Dependencies**: NautilusTrader Rust crates pinned by `Cargo.toml`, `serde`, `toml`, existing Bolt config, live-node, strategy-registration, test, and source-fence tooling.
**Storage**: In-memory live IV store with TOML-owned retention bounds; no durable storage in this feature.
**Testing**: `cargo test`, `cargo fmt --check`, `cargo clippy --locked --lib -- -D warnings`, `just source-fence`.
**Target Platform**: Existing pure Rust Bolt live binary and strategy runtime.
**Project Type**: Shared Rust crate module plus root config, live-node startup, strategy-registration/query-handle integration, tests, docs, and source-fence coverage.
**Performance Goals**: Store and policy operations are bounded by configured profile, source, series, strike, retention, interpolation, fallback, and quorum limits; no unbounded subscription or retention growth.
**Constraints**: No hardcoded strategy, venue, market, asset, cadence, instrument ID, source ID, timeout, quantity, or policy value. Runtime behavior comes from TOML. No Python layer. No FV/RV dependency. The active Speckit pointer for this planning slice points to `specs/026-nt-backed-iv-engine/`; any unrelated source-fence pointer policy must be handled as a separate gate before runtime implementation.
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
├── reference/
│   ├── overlap-ledger.md
│   ├── repository-truth.md
│   ├── evidence-ledger.md
│   ├── nt-evidence.md
│   ├── implementation-ledger.md
│   ├── internal-review.md
│   ├── external-review.md
│   └── final-summary.md
└── contracts/
    └── iv-engine-api.md
```

The repository's active `.specify/feature.json` and AGENTS Speckit block point at this IV packet for planning and task generation.
Reference files other than `overlap-ledger.md` are created or updated by the executable tasks in `tasks.md`.

### Source Code (repository root)

```text
src/bolt_v3_iv/
├── mod.rs                 # public IV engine module exports
├── capability.rs          # Cargo-pinned NT IV/options capability ledger
├── config.rs              # TOML-owned typed IV profile config
├── selector.rs            # typed source/query selector union
├── runtime.rs             # NT data actor/msgbus binding and event routing
├── subscription.rs        # NT subscribe/unsubscribe planning
├── ingest.rs              # raw NT/custom event ingestion
├── store.rs               # bounded raw and indexed IV store
├── raw_access.rs          # audit/replay-only raw payload access
├── query.rs               # strategy-facing IV query API
├── derive.rs              # NT helper-backed derived IV/greeks
├── policy.rs              # interpolation, fallback, quorum, projection, input, helper policy
├── provenance.rs          # required provenance schema
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
| "Use all NT offers" is unbounded | Add `capability.rs` and `tests/bolt_v3_iv_capability.rs`; the ledger resolves the NT checkout through `cargo metadata --locked` and `Cargo.lock`, then every discovered IV/options surface must be classified. |
| Capability discovery was too curated | Add a whole-checkout public-symbol candidate sweep for IV/options terms in addition to seed-family scans; unclassified candidates fail tests. |
| Subscribing to everything can explode cardinality | Support every NT source type, but subscribe only to TOML-configured clients, instruments, series, ranges, and selectors. |
| Derived IV needs complete inputs | `derive.rs` consumes `IvDerivedInputSet`; `policy.rs` resolves price, underlying, strike, side, time-to-expiry, rate, carry, source timestamps, and convention through `IvDerivedInputPolicy`. Missing/invalid inputs reject. |
| Venue/convention data must not be normalized away | Store raw NT payloads and index convention, basis, source, and provenance with every IV product. |
| Strategies need IV access but should not own mechanics | Expose indexed IV products through strategy-facing `query.rs`; expose raw payloads only through audit/replay `raw_access.rs`; source-fence rejects strategy-local NT IV subscriptions, raw payload audit readers, raw payload DTO imports, raw product queries, and helper-backed IV derivation globally in strategy modules. |
| Custom implied-volatility data is separate IV evidence | Model custom implied-volatility data as `IvEvidence`, not `IvPoint`, unless a configured projection explicitly derives an IV point. |
| NT timestamps are nanoseconds | Introduce typed timestamp handling and tests for freshness/retention/query conversion. |
| Option-chain lifecycle can leak stale surfaces | Add subscription lifecycle, unsubscribe, stale marking, and retention eviction tests. |
| Config shape can violate group-by-change | Replace separate source and strategy allow-list sections with one `IvProfile` boundary that owns sources, strategy authorization, lifecycle, and policies. |
| Plan could produce an isolated library | Add explicit edits and tests for `src/lib.rs`, root TOML loading, live-node startup, and strategy-registration query handles. |
| Aggregate greeks were claimed but not modeled | Add `IvAggregateGreeks`, raw aggregate preservation, indexed aggregate product tests, and query API coverage. |
| Interpolation/fallback/quorum were named but not testable | Add `policy.rs`, policy entities, fail-closed query behavior, and provenance requirements. |
| Custom evidence existed only in data-model | Add `IvEvidence` to the spec entity list and API product list. |
| Source-fence was conditional | Make strategy-owned NT IV subscriptions and NT helper-backed strategy-local IV derivation globally rejected in strategy modules. |
| Source-fence enforcement mechanism was unspecified | Add `tests/bolt_v3_iv_source_fence.rs` and wire it into the repository `source-fence` recipe so CI rejects strategy bypasses and IV core hardcodes. |
| Selector type system was missing | Add `selector.rs`, `IvSelector`, selector config validation, query validation, and mismatch tests for every source/product kind. |
| Provenance schema was undefined | Add `provenance.rs`, require `IvProvenance` on raw, indexed, derived, projected, policy, and rejected outputs, and fail tests when required fields are absent. |
| Runtime integration surface was hand-wavy | Add `runtime.rs` binding to NT data actor/msgbus subscription operations and event handlers; source health records subscription failures and stale generations. |
| Derived query inputs were missing | Add `IvDerivedInputPolicy` and `IvDerivedInputSet` to config, query, policy, and derive tests. |
| Projection policy entities were missing | Add `IvProjectionPolicy`; scalar IV from smile, surface, aggregate, or evidence products rejects without explicit projection. |
| Projection temporal skew was implicit | Add `max_projection_input_skew_ns`; projection tests reject cross-input timestamp skew violations. |
| Helper selection was implicit | Add `IvHelperPolicy` and helper provenance; derived-IV tests reject missing helper policy or incompatible helper signatures. |
| Policy provenance was opaque | Add typed `IvPolicyDecision` variants for projection, interpolation, fallback, quorum, helper, audit, and rejection paths. |
| Audit config existed only in quickstart | Add `IvAuditPolicy` to the data model, config, query boundary, and raw-access tests. |
| Strategy authorization granularity was ambiguous | Add `IvSelectorAuthorization` for profile-wide and selector-scoped strategy authorization. |
| Bounds and schema-version policies were under-specified | Add `IvNumericBounds`, convention bounds, accepted schema-version set, and version-bump migration rules. |
| Whole-checkout sweep terms could miss option-only vocabulary | Expand candidate terms to include strike, expiry, expiration, tenor, moneyness, skew, premium, and vol. |

## Workstreams

Each workstream follows `/Users/spson/Downloads/prompts/practice.md`: fetch/prune and record repository truth, produce a requirement evidence ledger, inspect pinned NT source before local helpers, name the TDD tests before implementation, run RED/GREEN/REFACTOR for behavior changes, and request external review only after exact-head local and CI evidence is green.

**W1 - NT capability ledger.** Build the source-backed inventory of NT IV/options surfaces at the Cargo-pinned revision. The test resolves the locked dependency graph with `cargo metadata --locked`, cross-checks NT package source revisions in `Cargo.lock`, locates the Cargo git checkout for that revision, scans model types, greeks helpers, msgbus APIs, data actor methods, data engine publish paths, option-chain manager, adapter support reachable through Rust, and custom implied-volatility data reachable through NT custom data as seed families. It also sweeps the full resolved NT checkout for public Rust symbols, modules, topics, and data definitions whose path, symbol, doc comment, or enclosing module matches IV/options terms. Gate: unclassified NT IV/options surfaces, unclassified whole-checkout sweep candidates, or unresolved Cargo evidence fail tests.

**W2 - Typed IV profile config.** Add TOML schema and validation for IV profiles that own schema version, schema-version policy, strategy authorization, selector authorization, audit policy, source IDs, data clients, source kinds, typed source selectors, typed query selector mappings, params, accepted conventions, IV bases, numeric/convention bounds, freshness, memory bounds, retention, projection, fallback, interpolation, extrapolation, quorum, helper, and derived-input policies. Gate: unknown schema versions and invalid TOML fail closed with exact field diagnostics, selector/source/product mismatches reject, and source rename fixtures prove group-by-change.

**W3 - Root config and live runtime wiring.** Export the IV module, load IV profiles from root TOML, start/stop the IV engine from live-node startup/shutdown, bind configured sources to NT data actor/msgbus subscription operations, route incoming events through raw preservation, and pass authorized IV query handles through strategy registration. Gate: live integration tests prove configured profiles produce live subscription plans, runtime bindings update source health, and strategies receive only authorized query handles.

**W4 - Subscription planner.** Convert typed profiles into NT subscribe/unsubscribe requests for option greeks, option chains, aggregate greeks, and custom implied-volatility data. Gate: a test data actor records the exact source kinds requested by each TOML fixture, including reload and source removal, and rejects unsupported runtime mappings.

**W5 - Raw ingestion and indexed products.** Ingest NT `OptionGreeks`, `OptionChainSlice`, aggregate greeks, and custom implied-volatility events. Preserve raw payloads for audit/replay access and build `IvPoint`, `IvGreeksPoint`, `IvAggregateGreeks`, `IvSmile`, `IvSurface`, `IvEvidence`, and `IvSourceHealth`. Gate: tests prove mark/bid/ask, greeks, convention, underlying price, open interest, timestamps, calls, puts, quotes, aggregate greeks, IV evidence values, source provenance, and audit raw retrieval are preserved without exposing raw payload DTOs to strategy handles.

**W6 - Strategy-facing query API and policies.** Expose IV point, greeks, aggregate greeks, smile, surface, evidence, source-health, scalar projection, interpolation, fallback, and quorum product queries to strategies. Keep raw payload retrieval on audit/replay handles only. Gate: multiple strategy harnesses use the same API with different profiles/selectors, profile-wide and selector-scoped authorization are both enforced, raw-payload product kinds reject on strategy handles, scalar projection without `IvProjectionPolicy` rejects, projection input skew rejects, and every policy decision records typed provenance or rejects.

**W7 - NT helper-backed derived IV.** Implement derived IV and derived greeks through NT math helpers only when `IvHelperPolicy` selects a ledger-supported helper and `IvDerivedInputSet` is complete and valid under `IvDerivedInputPolicy`. Gate: complete fixtures produce finite outputs; every missing helper policy, helper signature mismatch, missing/invalid input class, unresolved or expired rate/carry/time field, and stale/skewed input produces a typed rejection.

**W8 - Lifecycle, retention, and source-fence hardening.** Add unsubscribe, reload, stale, eviction, source removal, source-generation, raw-payload boundary, and direct-subscription/source-helper source-fence tests. Wire `tests/bolt_v3_iv_source_fence.rs` into `just source-fence`. Gate: stale or removed data cannot satisfy current queries, and strategy-local IV subscription mechanics, raw audit reader imports, raw IV DTO imports, raw payload product requests, helper-backed derivation, IV-shaped derivation from raw payload values, or IV core hardcodes fail source-fence.

## Phase Plan

### Phase 0: Research

Produce `research.md` with decisions for:

- Cargo-pinned NT capability inventory method.
- Whole-checkout NT IV/options candidate sweep method.
- Supported NT IV/options source kinds.
- Typed selector union.
- Strategy API exposure rule for product data and audit/replay-only raw data.
- IV profile config grouping and group-by-change validation.
- Root config, live-node, NT data actor/msgbus, and strategy-registration integration points.
- Timestamp representation and conversion policy.
- Derived-IV input contract.
- Projection policy contract.
- Provenance schema.
- Aggregate greeks indexed-product contract.
- Interpolation, fallback, extrapolation, and quorum policy contracts.
- Custom implied-volatility evidence classification.
- Retention and source lifecycle semantics.
- Source-fence enforcement mechanism.
- Active Speckit pointer disposition for this explicit IV packet.

### Phase 1: Design Contracts

Produce:

- `data-model.md` defining all IV entities and validation rules, including selector, provenance, projection, derived-input, runtime binding, and memory-bound entities.
- `contracts/iv-engine-api.md` defining strategy-facing API contracts, runtime binding, capability ledger, and source-fence boundaries.
- `quickstart.md` showing how an operator configures one IV profile without asset, venue, market, cadence, or concrete strategy-specific examples.

### Phase 2: Implementation Tasks

Generate `tasks.md` for this plan. Tasks must be TDD, dependency-ordered, independently reviewable by user story, and explicit about evidence collection from `practice.md`. No runtime code is written from this planning branch.

## Complexity Tracking

| Decision | Why needed | Simpler alternative rejected because |
|---|---|---|
| Full NT capability ledger | "All NT offers" must be testable | A hand-authored list will miss surfaces or drift after NT updates |
| Whole-checkout capability sweep | New NT IV/options families may appear outside seed paths | Seed-family scans alone could miss new public surfaces |
| Raw plus indexed store | Strategies need all NT data and generic products | Raw-only forces strategies to rebuild IV state; indexed-only loses NT data |
| Module directory | Capability, config, subscription, ingestion, store, query, derivation, and health are separate responsibilities | A single file would obscure boundaries and make review harder |
| Strategy source-fence | Strategies may consume IV but must not own mechanics | Relying on convention would recreate dual IV paths |
| Custom IV evidence type | Custom implied-volatility data is useful but not identical to option-chain IV | Treating it as a normal IV point would mislead consumers |
| IV profile boundary | Repo rules require group-by-change | Separate source and strategy-access sections make a source swap a multi-edit operation |
| Explicit policy module | Interpolation, fallback, and quorum can change answers | Leaving policy in prose would make strategy output non-auditable |
| Live integration workstream | The deliverable must subscribe through NT at runtime | An isolated module could pass unit tests without becoming the IV engine |
| Typed selector union | Different NT sources need different selector fields | Untyped maps make invalid source/query combinations runtime surprises |
| Required provenance schema | Raw, indexed, derived, and projected values are not interchangeable | Free-form provenance would leave audit gaps and source ambiguity |
| Explicit derived-input policy | NT helper outputs depend on rate, carry, time, and convention assumptions | Defaults would violate no-hardcodes and silently change IV |
| Audit-only raw payload access | Raw NT data must be preserved without creating strategy-local IV derivation | Strategy-dereferenceable raw payloads would bypass product provenance and source-fence |

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
