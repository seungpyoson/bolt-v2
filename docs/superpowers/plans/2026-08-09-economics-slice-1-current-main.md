# Economics Slice 1 Current-Main Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Atomically replace scalar fee authority with one typed, venue-agnostic economics quote used by current execution admission, evidence, and replay.

**Architecture:** A dependency-minimal `crates/economics-core` crate owns neutral contracts and calculations; `src/economics` is its sole Bolt facade. NautilusTrader/replay and venue adapters translate at the boundary, while current TOML composition selects exactly one venue adapter per execution client. The branch remains draft until every scalar-fee consumer is removed.

**Tech Stack:** Rust, `rust_decimal`, Serde/TOML at configuration boundaries, NautilusTrader Rust API in substrate adapters only.

## Global Constraints

- Start from authoritative current `main`; #1446 is reference-only.
- Shared core public types expose no NautilusTrader, Bolt runtime, venue SDK, transport, persistence, filesystem, database, or clock implementation types.
- `CurrencyId` and all other public identifiers are validated neutral domain values.
- Unknown, missing, stale, contradictory, unsupported, or unvalued required economics fails closed and never becomes zero.
- One production authority only: the final branch state contains no `FeeProvider`, provider fee builder, strategy fee math, or flat replay `fee_bps` path.
- Tests are behavioral or compiler-enforced; no source-scanning test is added.
- Live Polymarket and Hyperliquid execution remains disabled.
- Compile-heavy Cargo evidence is remote-first; any local Cargo target is `/Volumes/BoltCargoTarget/bolt-v2` on T9.

---

### Task 1: Compiler-isolated neutral economics core

**Files:**
- Create: `crates/economics-core/Cargo.toml`
- Create: `crates/economics-core/src/lib.rs`
- Create: `crates/economics-core/src/types.rs`
- Create: `crates/economics-core/src/quote.rs`
- Create: `crates/economics-core/src/edge.rs`
- Create: `crates/economics-core/src/valuation.rs`
- Create: `crates/economics-core/src/health.rs`
- Create: `src/economics/mod.rs`
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`

**Interfaces:**
- Produces: `CurrencyId`, `EconomicsInstrumentId`, `EconomicsQuoteRequest`, `EconomicsQuote`, `EstimatedEffect`, `EffectGuarantee`, `QuoteHealth`, `ValuationRate`, `DebitRiskBound`, and `fold_core_edge`.
- Consumes: only neutral decimal/value libraries.

- [x] Add behavior tests for identifier validation, signed native effects, health precedence, required valuation, debit bounds, and edge folding.
- [x] Implement validated neutral identifiers with private storage and fallible constructors.
- [x] Implement quote validation so required unknown or unvalued effects return a typed `EconomicsError`.
- [x] Implement edge folding that subtracts guaranteed costs and debit bounds and excludes forecast incentives from authorization.
- [x] Export the crate through `src/economics/mod.rs` and register the workspace dependency.
- [x] Run `cargo fmt --check` with the T9 target and `git diff --check`; rely on advisory CI for compilation.

### Task 2: NautilusTrader and replay request adapters

**Files:**
- Create: `src/integrations/nautilus/economics.rs`
- Modify: `src/integrations/nautilus/mod.rs`
- Create: `crates/backtesting-vertical-slice/src/economics.rs`
- Modify: `crates/backtesting-vertical-slice/src/lib.rs`
- Test: `tests/economics_nautilus_adapter.rs`
- Test: `crates/backtesting-vertical-slice/tests/economics_replay_adapter.rs`

**Interfaces:**
- Consumes: `EconomicsQuoteRequest` neutral contracts from Task 1 and current NT instrument/order/catalog facts.
- Produces: `economics_request_from_nt_intent` and `economics_request_from_replay_intent` with equivalent neutral semantics.

- [x] Add behavioral parity tests using the same intent represented by NT and replay fixtures.
- [x] Map NT identifiers, side, price, quantity, liquidity role, and observation time into neutral values.
- [x] Map replay catalog and manifest facts into the identical request without scalar fee defaults.
- [x] Reject missing price, quantity, role, currency, or observation time with typed adapter errors or unrepresentable typed inputs.

### Task 3: Venue economics adapters

**Files:**
- Create: `src/bolt_v3_providers/polymarket/economics.rs`
- Modify: `src/bolt_v3_providers/polymarket/mod.rs`
- Create: `src/bolt_v3_providers/hyperliquid/economics.rs`
- Modify: `src/bolt_v3_providers/hyperliquid/mod.rs`
- Test: `tests/polymarket_economics.rs`
- Test: `tests/hyperliquid_economics.rs`
- Add: focused JSON/TOML fixtures under `tests/fixtures/economics/`

**Interfaces:**
- Consumes: `EconomicsQuoteRequest` and authoritative venue schedule snapshots.
- Produces: one `VenueEconomicsAdapter` implementation per venue returning `EconomicsQuote`.

- [ ] Add Polymarket fixtures for fee-free schedules, maker/taker applicability, per-level rounding, unsupported exponents, routing charges, missing source, and stale source.
- [ ] Implement Polymarket estimates without treating NautilusTrader projection as authority.
- [ ] Add Hyperliquid fixtures for spot/perp maker/taker schedules, discounts, negative maker rates, side-dependent units, builder approval, carry bounds, aligned products, missing source, and schema divergence.
- [ ] Implement Hyperliquid estimates and block unrecognized `alignedQuoteTokenInfo` shapes.
- [ ] Prove both adapters use neutral identifiers and values at their public boundary.

### Task 4: Single TOML-backed composition route

**Files:**
- Modify: `src/bolt_v3_providers/mod.rs`
- Modify: `src/bolt_v3_strategy_registration.rs`
- Modify: `src/bolt_v3_strategy_context.rs`
- Modify: `src/bolt_v3_config.rs`
- Modify: current `config/root.toml` and `config/live.toml` without changing live/shadow posture
- Test: `tests/bolt_v3_provider_binding.rs`
- Test: `tests/bolt_v3_strategy_registration.rs`

**Interfaces:**
- Consumes: configured execution client and resolved authoritative venue inputs.
- Produces: exactly one `Arc<dyn VenueEconomicsAdapter>` per execution client in shared execution context.

- [ ] Add behavior tests for valid binding and fail-closed missing, duplicate, unsupported, or mismatched adapter configuration.
- [ ] Replace provider-registry `build_fee_provider` with one economics-adapter builder.
- [ ] Replace `StrategyBuildContext` fee authority with the shared execution economics handle; strategies cannot call venue formulas.
- [ ] Preserve existing live/shadow flags and reject configuration that would enable live execution.

### Task 5: Shared quote/admission and evidence cutover

**Files:**
- Modify: `src/bolt_v3_order_execution.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_sizing.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/runtime_state.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/strategies/binary_oracle_maker/mod.rs`
- Modify: `src/bolt_v3_current_evidence/facts.rs`
- Modify: `src/bolt_v3_current_evidence/codec.rs`
- Modify: `config/decision-evidence-contract.toml`
- Test: current execution, admission, maker, edge-taker, and evidence integration suites

**Interfaces:**
- Consumes: gross strategy intent plus one current `EconomicsQuote`.
- Produces: sized/admitted order intent whose evidence records quote identity, health, guaranteed effects, debit bounds, valuation lineage, and folded edge.

- [ ] Add fail-before-mutation tests for unavailable, stale, contradictory, unsupported, and unvalued required economics.
- [ ] Add tests proving forecast rewards do not improve admissible edge.
- [ ] Add resting-order tests proving material economics changes cause refresh.
- [ ] Move sizing and admission to the typed quote before identity mint, reservation, evidence authority, or dispatch mutation.
- [ ] Remove strategy-owned fee warm/read/math paths and pass gross assumptions only.
- [ ] Update typed evidence and codec fixtures without a parallel legacy fact.

### Task 6: Replay cutover and atomic legacy removal

**Files:**
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`
- Modify: `crates/backtesting-vertical-slice/src/run_manifest.rs`
- Modify: affected replay fixtures and reports
- Delete: obsolete provider fee implementations after all consumers move
- Modify: tests that currently construct `FeeProvider` doubles to construct typed economics adapters

**Interfaces:**
- Consumes: replay economics adapter from Task 2 and venue adapter from Task 3.
- Produces: replay decisions with the same quote/edge semantics as shared execution.

- [ ] Add replay parity tests for identical quote inputs and fail-closed unknown economics.
- [ ] Remove manifest `fee_bps` parsing and `ManifestFeeProvider`.
- [ ] Remove `FeeProvider`, `FeeProviderBuilder`, `resolve_fee_provider`, provider fee builders, strategy fee doubles, and duplicate scalar calculations.
- [ ] Verify by direct symbol inspection and compilation—not source-scanning tests—that no production scalar authority remains.
- [ ] Run format/static checks, push the exact head, and request the required reviewer; advisory CI supplies compile, Clippy, test, and build evidence.

## Later-slice ownership map

- Slice 2 owns the canonical actual economics ledger and booking lineage.
- Slice 3 owns supplemental venue actuals and reconciliation against canonical sources.
- Slice 4 owns lifecycle, carry, funding, borrow, and transfer actuals.
- Slice 5 owns reporting closure and estimate-versus-actual reconciliation views.

None of those later slices is implemented or enabled by this branch.
