# Issue 134 Runtime Enablement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement issue `#134` runtime enablement from `main@005d489` without adding any strategy logic.

**Architecture:** Replace the hardcoded `exec_tester` runtime path with explicit registry, snapshot, and fee-provider seams in small verified slices. Keep the first slice narrow, keep `main.rs`/`validate.rs`/runtime dispatch untouched until the plan reaches those items, and delete `exec_tester` only in a single focused purge near the end.

**Tech Stack:** Rust 2024, Nautilus Trader `af2aefc`, TOML config validation, NT msgbus, Polymarket Gamma + CLOB adapters.

---

### Task 1: Registry Contracts

**Files:**
- Create: `src/strategies/registry.rs`
- Modify: `src/strategies/mod.rs`
- Test: `src/strategies/registry.rs`

- [ ] Add `StrategyBuilder`, `StrategyRegistry`, `StrategyBuildContext`, and `BoxedStrategy` in `src/strategies/registry.rs`.
- [ ] Keep `StrategyBuildContext` limited to `fee_provider: Arc<dyn FeeProvider>`.
- [ ] Expose the registry module from `src/strategies/mod.rs` without registering any production kind.
- [ ] Add unit tests covering `register`, duplicate registration rejection, `get`, and sorted `kinds()`.
- [ ] Run `cargo test strategy_registry -- --nocapture`.

### Task 2: Test-Only Stub Runtime Strategy

**Files:**
- Modify: `tests/support/mod.rs`
- Create: `tests/support/stub_runtime_strategy.rs`
- Test: `tests/support/stub_runtime_strategy.rs`

- [ ] Add a minimal `StubRuntimeStrategy` implementing `StrategyBuilder`.
- [ ] Keep the helper private to `tests/support` and register it only inside tests.
- [ ] Add a second stub variant in test scope so registry dispatch can be proven with two kinds.
- [ ] Add tests showing test-only registry build/validate dispatch succeeds.
- [ ] Run `cargo test stub_runtime_strategy -- --nocapture`.

### Task 3: Polymarket Fee Seam

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/clients/polymarket.rs`
- Create: `src/clients/polymarket/fees.rs`
- Test: `src/clients/polymarket/fees.rs`

- [ ] Add `rust_decimal = "1.41.0"` if needed by the new fee-provider code.
- [ ] Add `FeeProvider` with sync `fee_bps` and async `warm`.
- [ ] Add `PolymarketClobFeeProvider` with 5-minute TTL, stale fallback, and cold-miss `None`.
- [ ] Add unit tests for cache hit, cold miss success, stale fallback, and cold miss plus fetch failure.
- [ ] Run `cargo test fee_provider -- --nocapture`.

### Task 4: Catalog Instrument Formatter And Candidate Shape

**Files:**
- Modify: `src/platform/polymarket_catalog.rs`
- Modify: `src/platform/ruleset.rs`
- Test: `tests/polymarket_catalog.rs`

- [ ] Replace `first_token_id` with centralized `polymarket_instrument_id(condition_id, token_id)`.
- [ ] Extend `CandidateMarket` to carry `condition_id`, `up_token_id`, `down_token_id`, and `start_ts_ms`.
- [ ] Remove any Gamma fee propagation from catalog output.
- [ ] Add or update tests proving instrument id round-trip and candidate translation shape.
- [ ] Run `cargo test polymarket_catalog -- --nocapture`.

### Task 5: Snapshot Envelope Before Runtime Rewrite

**Files:**
- Modify: `src/platform/ruleset.rs`
- Modify: `src/platform/runtime.rs`
- Test: `tests/platform_runtime.rs`

- [ ] Introduce `RuntimeSelectionSnapshot` and `platform.runtime.selection.<strategy_id>` topic helpers.
- [ ] Replace `RuntimeStrategyCommand::{Activate, Clear}` with snapshot publication only.
- [ ] Add tests proving snapshot shape/order without yet rewiring persistent lifecycle.
- [ ] Keep `exec_tester`, `main.rs`, and `validate.rs` unchanged through this task.
- [ ] Run `cargo test platform_runtime snapshot -- --nocapture`.

### Task 6: Mid-Stream Contract Check

**Files:**
- Review only

- [ ] Freeze the candidate head after Tasks 1 through 5.
- [ ] Run the dedicated Phase 2.5 contract review against the literal issue checkboxes.
- [ ] Fix any drift before proceeding.

### Task 7: Persistent Lifecycle Rewrite

**Files:**
- Modify: `src/platform/runtime.rs`
- Modify: `tests/platform_runtime.rs`
- Modify: `tests/live_node_run.rs`

- [ ] Rewrite runtime reconciliation to build once at startup and remove only at shutdown.
- [ ] Add persistent-lifecycle tests with `StubRuntimeStrategy` before deleting obsolete `exec_tester` lifecycle tests.
- [ ] Prove identical `component_id` across switch and idle transitions with zero `remove_strategy` calls before shutdown.
- [ ] Run `cargo test platform_runtime lifecycle -- --nocapture`.

### Task 8: Preemption And Running-State Gate

**Files:**
- Modify: `src/platform/runtime.rs`
- Modify: `tests/platform_runtime.rs`

- [ ] Add selector-side preemption via NT `cache.positions_open(None, None, Some(&strategy_id), None, None)`.
- [ ] Keep the selector idle until `LiveNodeHandle::state() == NodeState::Running`.
- [ ] Add tests for preemption suppression and `Starting -> Running` gating.
- [ ] Run `cargo test platform_runtime preemption -- --nocapture`.

### Task 9: Slug Matchers And Ruleset-Derived Data-Client Filters

**Files:**
- Modify: `src/clients/polymarket.rs`
- Modify: `src/validate.rs`
- Modify: `src/config.rs`
- Test: `src/validate/tests.rs`

- [ ] Replace plain `event_slugs` config with typed `SlugMatcher` values.
- [ ] Add `derive_event_slug_matchers(&RulesetConfig)` with explicit `event_slug_prefix`.
- [ ] Add the bolt-side refresh path needed because NT does not auto-refresh `update_instruments_interval_mins`.
- [ ] Add matcher derivation and prefix-expansion tests.
- [ ] Run `cargo test slug_matcher -- --nocapture`.

### Task 10: Registry Wiring In Validator And Main

**Files:**
- Modify: `src/validate.rs`
- Modify: `src/main.rs`
- Modify: `src/startup_validation.rs`
- Test: `src/validate/tests.rs`

- [ ] Derive valid kinds from `registry.kinds()`.
- [ ] Route kind-specific validation through the registry.
- [ ] Instantiate the fee provider in `main.rs` and use registry dispatch in strategy startup/runtime template lookup.
- [ ] Keep order-type policy out of `validate.rs`.
- [ ] Run `cargo test validate -- --nocapture`.

### Task 11: Atomic Exec Tester Purge

**Files:**
- Delete: `src/strategies/exec_tester.rs`
- Modify: `src/strategies/mod.rs`
- Modify: `src/live_config.rs`
- Modify: `Cargo.toml`
- Modify: `tests/platform_runtime.rs`
- Modify: `tests/polymarket_bootstrap.rs`
- Modify: `tests/live_node_run.rs`
- Modify: `tests/support/mod.rs`
- Modify: `tests/cli.rs`
- Modify: `tests/config_parsing.rs`
- Modify: `tests/config_schema.rs`
- Modify: `src/validate/tests.rs`
- Modify: `src/startup_validation.rs`
- Modify: `tests/raw_capture_transport.rs`

- [ ] Delete `exec_tester` production code in one focused change.
- [ ] Remove the live-config hardcode only; do not expand the materializer carve-out.
- [ ] Remove `nautilus-testkit` unless a remaining caller is proven.
- [ ] Delete obsolete destroy-on-switch / clear-on-idle / template-replacement tests and rewrite remaining coverage around the stub runtime strategy.
- [ ] Rewrite the raw-capture test fixture so the final grep gate does not match Gamma fee field reads outside the allowed documentation comment.
- [ ] Run `cargo test exec_tester -- --nocapture` and confirm zero remaining matches.

### Task 12: Final Grep Gates And Full Verification

**Files:**
- Modify: CI or local test support files only if required for assertions

- [ ] Add or script the Gamma-fee-field grep assertion.
- [ ] Add or script the `exec_tester`/`nautilus_testkit::testers` grep assertion.
- [ ] Run `cargo fmt --check`.
- [ ] Run `cargo clippy -- -D warnings`.
- [ ] Run `cargo deny check`.
- [ ] Run `cargo test`.

### Task 13: Review Lanes

**Files:**
- Review only

- [ ] Run the adversarial reviewer on the exact head and fix validated findings.
- [ ] Run the verifier on the exact head and collect command evidence.
- [ ] Run the code-quality reviewer on the exact head and fix low-cost confidence debt.
- [ ] Summarize remaining Issue 2 handoff assumptions only after fresh verification passes.
