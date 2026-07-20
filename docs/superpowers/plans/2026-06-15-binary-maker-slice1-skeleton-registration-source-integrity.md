# Binary-Maker Slice 1 — Inert Maker Skeleton + Injectable Registration + MAKER_KEY Source Integrity — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Rust verification is **CI-only** (local cargo refused by `ci/rust-verification.toml [local_compile_policy]`); "run the test" steps for Rust mean *use `git push` and let advisory CI evaluate the exact head*. Python fences run locally.

**Goal:** Land a registered-but-**inert** `BinaryOracleMaker` strategy that compiles, is selectable by config archetype key, and carries its own `MAKER_KEY` source-integrity digest — proving the registration + source-integrity path end-to-end **before any maker behavior**. Closes §16#1 (injectable binding) + §16#2 (`MAKER_KEY` golden digest, `STRATEGY_KEY` untouched).

**Architecture:** The maker is injected through the **already-generic** registration seam — it is NOT a copy of the taker archetype. The maker strategy + builder + archetype binding live entirely in the **non-scanned** `src/strategies/binary_oracle_maker/` layer. The production binding *lists* are **hoisted** out of the scanned `src/bolt_v3_archetypes/mod.rs` into a new **non-`bolt_v3_`-prefixed crate-root module `src/strategy_bindings.rs`** that may reference both the taker's (`crate::bolt_v3_archetypes::…`) and the maker's (`crate::strategies::…`) bindings. The two scanned call sites (`live_node.rs`, `bolt_v3_validate.rs`) are repointed to `crate::strategy_bindings::…` — a module whose root is `strategy_bindings`, **not** `strategies`, so the dependency-direction fence (forbidden root `STRATEGY_ROOT = "strategies"`, `verify_bolt_v3_dependency_direction.py:93`) does not flag it and **no new `FINDING_ALLOWANCES` are added** (the shrink-only fence would otherwise fail).

**Tech Stack:** Rust + NautilusTrader Rust API (rev pinned by `Cargo.toml`); Python CI fences; `just` recipes.

**Source spec:** `docs/superpowers/specs/2026-06-14-multi-asset-mm-platform-design.md` (§16#1, §16#2). Program: `docs/superpowers/plans/2026-06-15-binary-maker-program.md`. Issue **#488**.

**Branch:** `feat/488-slice1-maker-skeleton` stacked on `feat/488-slice0-fences-feed-health` (Slice 0 not yet merged; rebase onto `main` after Slice 0 lands). **Depends:** Slice 0.

---

## Verified anchors (re-read at branch HEAD `839ef3552` before editing — anchors drift)

| Symbol | Location | Role |
|---|---|---|
| `StrategyRuntimeBinding { key, strategy_kind, register }` | `src/bolt_v3_strategy_registration.rs:18-26` | the injectable binding struct |
| `register_bolt_v3_strategies_on_node_with_bindings(node, loaded, resolved, bindings, …)` | `src/bolt_v3_strategy_registration.rs:108` | generic core; matches `binding.key == strategy.strategy_archetype`; never names an archetype |
| taker `RUNTIME_BINDING` const | `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:87-91` | `{ key: KEY, strategy_kind: …Builder::kind, register: register_runtime_strategy }` |
| taker `register_runtime_strategy` | `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:400-444` | resolves fee/venue, builds `StrategyBuildContext`, calls `production_strategy_registry().register_strategy(...)` — **the pattern the maker mirrors STRUCTURALLY (not copies)** |
| `RUNTIME_BINDINGS` / `runtime_bindings()` | `src/bolt_v3_archetypes/mod.rs:65,71` | production list (to be hoisted out) |
| `VALIDATION_BINDINGS` / `validation_bindings()` / `ArchetypeValidationBinding` | `src/bolt_v3_archetypes/mod.rs:55-58,60-63,67-69` | validation list (to be hoisted out) |
| `validate_strategy_archetype_with_bindings(...)` | `src/bolt_v3_archetypes/mod.rs:88` | generic validation dispatch (KEPT in archetypes; takes the list as a param) |
| runtime call site | `src/bolt_v3_live_node.rs:1880,1884` | passes `crate::bolt_v3_archetypes::runtime_bindings()` → **repoint** |
| validation call site | `src/bolt_v3_validate.rs:1881` | calls `crate::bolt_v3_archetypes::validate_strategy_archetype(...)` → **repoint** |
| injectability proof (test) | `tests/bolt_v3_strategy_registration.rs:499` | already passes a hand-written bindings slice through the core fn |
| `StrategyBuilder { kind, validate_config, build, register }` | `src/strategies/registry.rs:223-232` | generic factory trait |
| `production_strategy_registry()` | `src/strategies/mod.rs:8-12` | registers builders; **non-scanned**; add the maker here |
| taker NT surface (struct+`core`/`new`/`impl DataActor`/`nautilus_strategy!`) | `src/strategies/binary_oracle_edge_taker/mod.rs:880-936,4952,5225,5337-5413` | the inert-skeleton shape to follow |
| `GATED_SOURCE_ROOTS` + `STRATEGY_KEY`/`SUBMIT_ADMISSION_KEY` | `src/source_canonicalization.rs:541-590` | the ONE digest registry (flat array); add a 3rd entry |
| `GOLDEN_STRATEGY_DIGEST` + value-stability test | `src/bolt_v3_source_integrity.rs:287-288,310-317` | the golden-gate pattern to mirror |
| Python registry mirror | `scripts/bolt_v3_source_roots.py:28-44` | must add `MAKER_SOURCE_ROOTS` |
| Python↔Rust coupling test (fail-closed) | `scripts/test_verify_bolt_v3_legacy_default_fence.py:241-253` | expected set MUST include maker roots |
| dependency fence forbidden root | `scripts/verify_bolt_v3_dependency_direction.py:90,93` | `SCAN_PREFIX="src/bolt_v3_"`, `STRATEGY_ROOT="strategies"` |

**Anchor discipline:** the implementer MUST `git show 839ef3552:<file>` (or read at the executor HEAD) each cited region before editing; line numbers above are from the planning HEAD and may have shifted.

---

## File structure

**New (all in the non-scanned strategy layer + one crate-root module):**
- `src/strategies/binary_oracle_maker/mod.rs` — `BinaryOracleMaker` struct (`core: StrategyCore`, `#[derive(Debug)]`), `new()`, **empty** `impl DataActor for BinaryOracleMaker {}`, `nautilus_strategy!(BinaryOracleMaker);`, `pub const KEY: &str = "binary_oracle_maker";`, and the `StrategyBuilder` impl.
- `src/strategies/binary_oracle_maker/config.rs` — `pub struct BinaryOracleMakerBuilder;` + minimal `BinaryOracleMakerConfig` + `parse_config`/`validate_config`.
- `src/strategies/binary_oracle_maker/archetype.rs` — the maker's `validate_strategy`, `register_runtime_strategy`, and `pub const RUNTIME_BINDING: StrategyRuntimeBinding` (lives in non-scanned land so it may reference `crate::strategies::binary_oracle_maker::*`).
- `src/strategy_bindings.rs` — **the hoisted non-scanned aggregator**: `production_runtime_bindings()` and `production_validation_bindings()`.

**Edits (additive / repoint only):**
- `src/lib.rs` — `pub mod strategy_bindings;` (alphabetical, crate-root block).
- `src/strategies/mod.rs` — `pub mod binary_oracle_maker;` + register `BinaryOracleMakerBuilder` in `production_strategy_registry()`.
- `src/bolt_v3_archetypes/mod.rs` — **remove** `RUNTIME_BINDINGS`/`runtime_bindings()`/`VALIDATION_BINDINGS`/`validation_bindings()`/`validate_strategy_archetype()` (the production wrappers move to the aggregator); **keep** the types (`ArchetypeValidationBinding`, gate enums), `validate_strategy_archetype_with_bindings`, and `pub mod binary_oracle_edge_taker;` (so the aggregator can reach the taker binding). No dead code left behind (NO DEBTS).
- `src/bolt_v3_live_node.rs:1884` — `crate::bolt_v3_archetypes::runtime_bindings()` → `crate::strategy_bindings::production_runtime_bindings()` (+ fix the `use` at `:100-101`).
- `src/bolt_v3_validate.rs:1881` — `crate::bolt_v3_archetypes::validate_strategy_archetype(ctx, s, cap)` → `crate::bolt_v3_archetypes::validate_strategy_archetype_with_bindings(ctx, s, cap, crate::strategy_bindings::production_validation_bindings())`.
- `src/source_canonicalization.rs` — `pub const MAKER_KEY: &str = "maker";` + 3rd `GatedSourceRoot` entry (maker dir only; STRATEGY_KEY roots byte-identical).
- `src/bolt_v3_source_integrity.rs` — re-export `MAKER_KEY`; add `GOLDEN_MAKER_DIGEST` + `value_stability_maker_digest_equals_golden_constant` test.
- `scripts/bolt_v3_source_roots.py` — add `MAKER_SOURCE_ROOTS`.
- `scripts/test_verify_bolt_v3_legacy_default_fence.py:247-253` — include `*MAKER_SOURCE_ROOTS` in the expected set (mandatory; key-agnostic Rust parser will otherwise fail-close).

**No edit needed:** `build.rs` (iterates `GATED_SOURCE_ROOTS` generically), `src/bolt_v3_strategy_registration.rs` (core is archetype-agnostic), the dependency-fence `FINDING_ALLOWANCES` (design adds zero `crate::strategies` refs in scanned files).

---

## Unit A — Inert maker strategy + builder (compiles, unit-registers)

### Task A1: maker config + builder

**Files:** Create `src/strategies/binary_oracle_maker/config.rs`; (Task A3 wires the module).

- [ ] **Step 1 — read the taker convention.** `git show 839ef3552:src/strategies/binary_oracle_edge_taker/config.rs` around the `BinaryOracleEdgeTakerBuilder` struct (≈`:253`) and its `parse_config`/`validate` to mirror the *shape*. Identify the **minimal** `BinaryOracleMakerConfig` fields the NT `StrategyConfig` envelope requires (`strategy_id`, `order_id_tag`, `oms_type`, plus whatever `StrategyCore::new` consumes — read `src/strategies/binary_oracle_edge_taker/mod.rs:885-936`). NO trading parameters (inert).
- [ ] **Step 2 — write `config.rs`:** `pub struct BinaryOracleMakerConfig { … minimal envelope fields … }` deriving `serde::Deserialize`; `pub struct BinaryOracleMakerBuilder;`; `pub fn parse_config(raw: &serde_json::Value) -> Result<BinaryOracleMakerConfig, …>`; `pub fn validate_config(raw, ctx, errors: &mut Vec<ValidationError>)` performing only envelope validation. All values from the parsed config (NO HARDCODES).
- [ ] **Step 3 — failing test (CI):** add `src/strategies/binary_oracle_maker/tests.rs` (or inline `#[cfg(test)]`) asserting `parse_config` round-trips a minimal TOML/JSON envelope and `validate_config` returns no errors for it. Push → CI compiles+runs (local cargo refused).

### Task A2: inert `BinaryOracleMaker` + NT surface + `StrategyBuilder`

**Files:** Create `src/strategies/binary_oracle_maker/mod.rs`.

- [ ] **Step 1 — read the minimal NT surface:** `src/strategies/binary_oracle_edge_taker/mod.rs:880-936` (struct + `new` + `StrategyCore::new(StrategyConfig{…})`), `:5225` (`nautilus_strategy!` use), `:5337-5413` (`KEY` + `StrategyBuilder` impl). NT facts (verified): `Strategy: DataActor`; `DataActor` handlers all default to no-op; `nautilus_strategy!($ty);` supplies `core()/core_mut()` + glue; bound for `add_strategy` is `Strategy + Component + Debug + 'static`.
- [ ] **Step 2 — write the inert strategy:**
  - `#[derive(Debug)] pub struct BinaryOracleMaker { core: StrategyCore, config: BinaryOracleMakerConfig, context: StrategyBuildContext }`
  - `impl BinaryOracleMaker { pub fn new(config, context) -> Self }` building `StrategyCore::new(StrategyConfig { strategy_id: Some(StrategyId::from(<archetype>-<order_id_tag>)), order_id_tag, oms_type, … })` mirroring taker `:919-936` but with **no** active/pricing/exposure state.
  - `impl DataActor for BinaryOracleMaker {}` — **empty** (every handler defaults → subscribes to nothing, emits no orders → inert).
  - `nautilus_strategy!(BinaryOracleMaker);` — **no** extra-items block.
  - `pub const KEY: &str = "binary_oracle_maker";`
  - `impl StrategyBuilder for BinaryOracleMakerBuilder { fn kind()->&'static str { KEY } fn validate_config(...) { config::validate_config(...) } fn build(raw,ctx)->Result<BoxedStrategy> { Ok(Box::new(BinaryOracleMaker::new(config::parse_config(raw)?, ctx.clone()))) } fn register(raw,ctx,trader)->Result<StrategyId> { let s = BinaryOracleMaker::new(config::parse_config(raw)?, ctx.clone()); let id = StrategyId::from(s.component_id().inner().as_str()); trader.borrow_mut().add_strategy(s)?; Ok(id) } }` — mirroring taker `:5365-5413` exactly in shape.
- [ ] **Step 3 — failing test (CI):** assert `BinaryOracleMakerBuilder::kind() == "binary_oracle_maker"` and that `build(minimal_raw, &ctx)` returns `Ok`. Push → CI.
- [ ] **Step 4 — commit** (`feat(488): inert BinaryOracleMaker strategy + builder`).

### Task A3: wire the module + reuse the production registry (no dual path)

**Files:** `src/strategies/mod.rs`.

- [ ] **Step 1 — read** `src/strategies/mod.rs:1-12` (`pub mod …;` block + `production_strategy_registry()` body registering the taker via `registry.register::<…Builder>()`).
- [ ] **Step 2 — edit:** add `pub mod binary_oracle_maker;` and, in `production_strategy_registry()`, add `registry.register::<binary_oracle_maker::BinaryOracleMakerBuilder>()?;` (one additive line, alongside the taker — reuses the ONE registry).
- [ ] **Step 3 — failing test (CI):** assert `production_strategy_registry()` contains the maker kind (mirror however the taker asserts its own registration). Push → CI. **Commit.**

---

## Unit B — Injectable binding hoist + maker archetype binding + repoints (§16#1)

### Task B1: maker archetype binding in non-scanned land

**Files:** Create `src/strategies/binary_oracle_maker/archetype.rs`.

- [ ] **Step 1 — read the taker binding:** `src/bolt_v3_archetypes/binary_oracle_edge_taker.rs:55-95` (imports + `RUNTIME_BINDING` const) and `:400-444` (`register_runtime_strategy`: resolve fee_provider+execution_venue from config, build `StrategyBuildContext::new(...)`, call `production_strategy_registry().register_strategy(KEY, &raw, &ctx, node.kernel().trader())`). Also read `src/bolt_v3_archetypes/mod.rs:55-63` for `ArchetypeValidationBinding`/`validate_strategy` shapes.
- [ ] **Step 2 — write `archetype.rs`** (this file is under `src/strategies/` → non-scanned → may freely reference `crate::strategies::binary_oracle_maker::*`, `crate::strategies::production_strategy_registry`, and the `bolt_v3_*` registration/live-node types):
  - `pub fn validate_strategy(context: &str, strategy: &BoltV3StrategyConfig, default_max_notional_decimal: Option<&Decimal>) -> Vec<String>` — minimal envelope validation for the inert maker (no parameter-row rules yet).
  - `pub fn register_runtime_strategy(node: &mut LiveNode, ctx: StrategyRegistrationContext) -> Result<StrategyId, BoltV3StrategyRegistrationError>` — mirror the taker's structure with the **minimal** `StrategyBuildContext` + raw table the inert maker needs.
  - `pub const RUNTIME_BINDING: StrategyRuntimeBinding = StrategyRuntimeBinding { key: super::KEY, strategy_kind: BinaryOracleMakerBuilder::kind, register: register_runtime_strategy };`
- [ ] **Step 3 — failing test (CI):** unit-assert `RUNTIME_BINDING.key == "binary_oracle_maker"`. Push → CI.

### Task B2: the hoisted non-scanned aggregator

**Files:** Create `src/strategy_bindings.rs`; `src/lib.rs`.

- [ ] **Step 1 — write `src/strategy_bindings.rs`** (crate root, **not** `bolt_v3_`-prefixed → non-scanned; root `strategy_bindings` ≠ `strategies` → callable from scanned files):
```rust
//! Production strategy-archetype binding lists, assembled in a NON-scanned
//! crate-root module so it may name both the shared-layer (`crate::bolt_v3_*`)
//! and strategy-layer (`crate::strategies::*`) bindings without violating the
//! dependency-direction fence (forbidden root is `strategies`, not this module)
//! and without growing `FINDING_ALLOWANCES`.
use crate::bolt_v3_archetypes::{binary_oracle_edge_taker, ArchetypeValidationBinding};
use crate::bolt_v3_strategy_registration::StrategyRuntimeBinding;
use crate::strategies::binary_oracle_maker;

const RUNTIME_BINDINGS: &[StrategyRuntimeBinding] = &[
    binary_oracle_edge_taker::RUNTIME_BINDING,
    binary_oracle_maker::archetype::RUNTIME_BINDING,
];

const VALIDATION_BINDINGS: &[ArchetypeValidationBinding] = &[
    ArchetypeValidationBinding { key: binary_oracle_edge_taker::KEY, validate_strategy: binary_oracle_edge_taker::validate_strategy },
    ArchetypeValidationBinding { key: binary_oracle_maker::KEY, validate_strategy: binary_oracle_maker::archetype::validate_strategy },
];

pub fn production_runtime_bindings() -> &'static [StrategyRuntimeBinding] { RUNTIME_BINDINGS }
pub fn production_validation_bindings() -> &'static [ArchetypeValidationBinding] { VALIDATION_BINDINGS }
```
  (Confirm `ArchetypeValidationBinding` + `binary_oracle_edge_taker::{KEY,validate_strategy,RUNTIME_BINDING}` are `pub`-reachable from the aggregator; if `mod.rs` currently keeps them `pub(crate)`/private, widen to `pub` as needed.)
- [ ] **Step 2 — `src/lib.rs`:** add `pub mod strategy_bindings;` in the crate-root module block.
- [ ] **Step 3 — remove the now-orphaned production wrappers** from `src/bolt_v3_archetypes/mod.rs` (`RUNTIME_BINDINGS`, `runtime_bindings()`, `VALIDATION_BINDINGS`, `validation_bindings()`, `validate_strategy_archetype()`), keeping `validate_strategy_archetype_with_bindings`, the types, and `pub mod binary_oracle_edge_taker;`. (NO DEBTS — no dead code.)

### Task B3: repoint the two scanned call sites + run all fences

**Files:** `src/bolt_v3_live_node.rs`, `src/bolt_v3_validate.rs`.

- [ ] **Step 1 — `live_node.rs`:** change `:1884` to `crate::strategy_bindings::production_runtime_bindings()` and fix the `use` at `:100-101` (drop the archetypes `runtime_bindings` import if now unused).
- [ ] **Step 2 — `bolt_v3_validate.rs:1881`:** change to `crate::bolt_v3_archetypes::validate_strategy_archetype_with_bindings(ctx, s, cap, crate::strategy_bindings::production_validation_bindings())`.
- [ ] **Step 3 — fences (local Python, authoritative for direction):**
  - `python3 scripts/verify_bolt_v3_dependency_direction.py` → OK (confirms `strategy_bindings`/`live_node`/`validate` add **no** `crate::strategies` reference in scanned files).
  - `python3 scripts/verify_bolt_v3_dependency_direction.py --check-shrink-only-vs-main` → PASS, `FINDING_ALLOWANCES` **unchanged** (zero growth). If this fails, a scanned file is referencing `crate::strategies` — fix the design, do NOT add an allowance.
  - `python3 scripts/verify_bolt_v3_no_venue_name_branch.py` → OK.
- [ ] **Step 4 — registration test (CI):** extend/mirror `tests/bolt_v3_strategy_registration.rs` to assert a loaded config with `strategy_archetype = "binary_oracle_maker"` registers via `production_runtime_bindings()` and validates via `production_validation_bindings()` (the taker still registers — both keys present). Push → CI (full `register_bolt_v3_strategies_on_node_with_bindings` path).
- [ ] **Step 5 — commit** (`feat(488): hoist strategy binding lists to non-scanned aggregator; inject maker (§16#1)`).

---

## Unit C — `MAKER_KEY` source integrity (§16#2)

### Task C1: add MAKER_KEY to the Rust registry (additive; STRATEGY_KEY untouched)

**Files:** `src/source_canonicalization.rs`, `src/bolt_v3_source_integrity.rs`.

- [ ] **Step 1 — read** `src/source_canonicalization.rs:541-590` (the `STRATEGY_KEY`/`SUBMIT_ADMISSION_KEY` consts + the `GATED_SOURCE_ROOTS` array literal).
- [ ] **Step 2 — edit `source_canonicalization.rs`:** add `pub const MAKER_KEY: &str = "maker";` next to the other key consts, and **append** a 3rd array element `GatedSourceRoot { key: MAKER_KEY, relative_roots: &["src/strategies/binary_oracle_maker"] }` (maker dir only — do NOT gate the unwired helper libs yet; they are not consumed until later slices). Leave the `STRATEGY_KEY` entry byte-identical (its digest must not rotate).
- [ ] **Step 3 — `bolt_v3_source_integrity.rs`:** re-export `MAKER_KEY` in the `pub use` block (≈`:25`). Inside `#[cfg(test)] mod tests`, add `const GOLDEN_MAKER_DIGEST: &str = "<placeholder>";` (with a one-line provenance comment) and `value_stability_maker_digest_equals_golden_constant` mirroring `:310-317` (computes `registry_source_digest(MAKER_KEY, TEST_MAX_BYTES)`).
- [ ] **Step 4 — regenerate the golden (CI):** push; read the actual digest from the failing `assert_eq` `left` value in CI output; paste it into `GOLDEN_MAKER_DIGEST`; push again → green. (This is the established workflow the ~140 `Re-derived` provenance comments document; there is no codegen script.)

### Task C2: mirror in Python + the fail-closed coupling test

**Files:** `scripts/bolt_v3_source_roots.py`, `scripts/test_verify_bolt_v3_legacy_default_fence.py`.

- [ ] **Step 1 — read** `scripts/bolt_v3_source_roots.py:28-44` and `scripts/test_verify_bolt_v3_legacy_default_fence.py:24-30,241-253` (the key-agnostic Rust-registry parser + the expected-set assertion).
- [ ] **Step 2 — `bolt_v3_source_roots.py`:** add `MAKER_SOURCE_ROOTS = ("src/strategies/binary_oracle_maker",)` mirroring the Rust roots exactly.
- [ ] **Step 3 — `test_verify_bolt_v3_legacy_default_fence.py:247-253`:** add `*source_roots.MAKER_SOURCE_ROOTS` to the expected set (MANDATORY — the Rust-registry parser is key-agnostic and will see the new maker roots; without this the coupling test fails closed).
- [ ] **Step 4 — run locally:** `python3 scripts/test_verify_bolt_v3_legacy_default_fence.py` → OK (Rust registry roots set == Python roots set).
- [ ] **Step 5 — commit** (`feat(488): MAKER_KEY source-integrity digest + Python mirror (§16#2)`).

---

## Self-Review

**Spec coverage:** §16#1 (injectable binding hoisted to non-scanned caller, NOT a taker mirror) → Unit B (`strategy_bindings.rs` aggregator + maker archetype in `src/strategies/`). §16#2 (`MAKER_KEY` golden digest, `STRATEGY_KEY` untouched) → Unit C (additive `GatedSourceRoot`, disjoint root set). Inert registered skeleton → Units A+B. ✓

**Fence interaction (load-bearing):** every scanned (`src/bolt_v3_*`) edit references only `crate::strategy_bindings` (root ≠ `strategies`) or `bolt_v3_*` symbols — zero new `crate::strategies` refs → `--check-shrink-only-vs-main` stays at baseline (verified in Task B3). The maker archetype binding lives in `src/strategies/` (non-scanned) precisely so it may name `crate::strategies::binary_oracle_maker`. ✓

**No dual path:** the maker reuses `production_strategy_registry()`, `StrategyBuilder`, `StrategyRuntimeBinding`, and `register_bolt_v3_strategies_on_node_with_bindings` — no parallel registry. The hoist MOVES the production lists (removing the archetypes wrappers); it does not duplicate them. ✓

**Type consistency:** `KEY = "binary_oracle_maker"` is used identically as `StrategyBuilder::kind`, archetype binding key, `RUNTIME_BINDING.key`, validation binding key, and the operator TOML `strategy_archetype` value. `MAKER_KEY = "maker"` is the digest-registry key only (distinct from the archetype key, mirroring the taker's `STRATEGY_KEY="strategy"` vs archetype `KEY="binary_oracle_edge_taker"` split). ✓

**Inert guarantee:** empty `impl DataActor {}` + bare `nautilus_strategy!` ⇒ no subscriptions, no orders. The strategy registers and validates but does nothing until later slices add handlers. ✓
