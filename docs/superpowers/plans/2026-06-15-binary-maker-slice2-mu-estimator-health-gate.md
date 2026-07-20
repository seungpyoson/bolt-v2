# Binary-Maker Slice 2 — μ Estimator + Health Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the binary-oracle maker a net-new informed-fraction (μ) estimator over the existing signed trade-flow buffer, plus a fail-closed μ-health gate that blocks quoting (future) AND go-live (now) when μ is absent, stale, non-finite, or degenerate (constant-0).

**Architecture:** Two pure shared functions in a new ungated module `src/bolt_v3_maker_mu_estimator.rs` (sibling to the already-shared `bolt_v3_trade_flow.rs` / `bolt_v3_maker_model.rs`): `estimate_informed_fraction` (VPIN single-window order-flow-imbalance magnitude over `SignedTradeFlow::samples_within`) and `evaluate_mu_health` (→ `Option<MuHealthReason>`). The per-strategy runtime state + wiring lives under the gated maker dir in a new `src/strategies/binary_oracle_maker/mu.rs` (owns a `BTreeMap<InstrumentId, SignedTradeFlow>`, observes trades, derives μ + health per instrument). The maker config gains μ knobs (taker-pattern config-struct fields, not a `[parameters]` table); `validate_config` enforces the fail-closed go-live contract (μ source must be enabled with in-range knobs). The maker's `on_trade` feeds the buffers. No orders are emitted (quoting is Slice 6).

**Tech Stack:** Rust, NautilusTrader Rust API (rev `6e059dc`), TOML config, Python CI fences, `just`.

**Slice scope (§16#6).** Ships: the μ estimator + μ-health gate + their config + the maker `on_trade` feeding seam. Out of scope (stated explicitly): the live trade *subscription* that delivers ticks to `on_trade` rides on the maker's book/market subscription, which is built in **Slice 6/9** — Slice 2 builds the handler + buffers + estimator + gate + config + the go-live structural gate; the subscription's instrument set is empty until selection lands. The μ→GM wiring (`gm_binary_quote(p, μ)` as sole producer of the reservation band) is **Slice 3**. This is partial scope by design and is tracked in PR #716's slice checklist.

**Source-integrity note (MANDATORY, up front).** Every byte change under `src/strategies/binary_oracle_maker/` rotates `GOLDEN_MAKER_DIGEST` (`src/bolt_v3_source_integrity.rs`). The new `src/bolt_v3_maker_mu_estimator.rs` is a crate-root shared module (like `bolt_v3_trade_flow.rs`) and is NOT under any gated key, so it does NOT rotate any seal. Re-record `GOLDEN_MAKER_DIGEST` as the LAST code task, from the CI value-stability test's surfaced value (Task 7). Do not hand-compute it; do not put the hex in a commit message (credential-scanner false positive).

---

## Grounded anchors (verified at HEAD `c12b017be`)

- `src/bolt_v3_trade_flow.rs` — `SignedTrade { ts_ms: u64, aggressor: AggressorSide, price: f64, size: f64 }` (`:13-19`); `SignedTradeFlow::samples_within(now_ms) -> impl Iterator<Item=&SignedTrade>` (`:117-122`); `samples() -> &VecDeque<SignedTrade>` (`:108-110`); `from_config`/`observe` are `pub(crate)` (`:48,57`). Buffer drops non-monotonic-ns observations silently (`:63-68`) and excludes future/aged trades in `samples_within`.
- `nautilus_model::enums::AggressorSide` — `NoAggressor = 0` (`#[default]`), `Buyer = 1`, `Seller = 2`. Polymarket adapter populates Buyer/Seller (never NoAggressor) for live trades; NoAggressor only from default-constructed / replay ticks.
- `src/bolt_v3_maker_model.rs:51-94` — `gm_binary_quote(fair_p_up, informed_fraction) -> Option<BinaryGmQuote>`; accepts μ=0 → bid=ask=fair (zero spread). The μ-health gate is the only barrier against degenerate μ. (Slice 3 wires this consumer; Slice 2 only produces μ + the gate.)
- `src/strategies/binary_oracle_maker/mod.rs` — inert `BinaryOracleMaker { core, config }`, empty `impl DataActor for BinaryOracleMaker {}` (`:73`), `new(config)` (`:48`), `nautilus_strategy!` (`:75`), `KEY` (`:36`).
- `src/strategies/binary_oracle_maker/config.rs` — `BinaryOracleMakerConfig { strategy_id, order_id_tag, oms_type }` (`#[serde(deny_unknown_fields)]`); `parse_config` (`:52`), `validate_config` (`:61-106`: table check, unknown-key loop vs known names, per-field string check, `validate_oms_type_parses`).
- `src/strategies/binary_oracle_maker/archetype.rs:55-78` — `validate_strategy`; currently rejects any non-empty `[parameters]` table (the inert policy). Slice 2 keeps `[parameters]` rejected (μ knobs live on the config struct, taker-pattern) — confirm the taker pattern in Task 4.
- `src/source_canonicalization.rs` — `MAKER_KEY = "maker"` (`:545`), `GatedSourceRoot` for MAKER_KEY = `&["src/strategies/binary_oracle_maker"]` (`:584-587`). STRATEGY_KEY roots do NOT include the maker dir; the spec forbids expanding STRATEGY_KEY for the maker.
- `src/bolt_v3_source_integrity.rs:309-314` — `GOLDEN_MAKER_DIGEST` constant; `:339-345` value-stability test asserting `registry_source_digest(MAKER_KEY,…) == GOLDEN_MAKER_DIGEST`.
- `src/lib.rs:38` — `pub mod bolt_v3_trade_flow;` (the sibling-tier insertion point for the new module decl).
- Dependency-direction fence (`scripts/verify_bolt_v3_dependency_direction.py`): files matching `src/bolt_v3_*` MUST NOT reference `crate::strategies`. The new estimator module is `src/bolt_v3_*` → it MUST NOT import anything from `crate::strategies`. `strategies → bolt_v3_*` (the maker importing the estimator) is allowed.

---

## Design decisions (locked)

**D1 — μ estimator (VPIN single-window order-flow imbalance).**
μ = |buy_volume − sell_volume| / (buy_volume + sell_volume) over `samples_within(now_ms)`, counting only `Buyer`/`Seller` aggressors (exclude `NoAggressor` — fail-closed, never treat unknown as net-zero or as a side). Bounded [0,1]: 0 = perfectly balanced flow (no directional information), 1 = fully one-sided (maximally toxic). This is the informed-fraction μ that `gm_binary_quote` consumes. Returns `Option<f64>`:
- `None` if classified-sample count < `min_classified_samples` (warming up / insufficient), OR total classified volume not `> 0.0`, OR the result is non-finite.
- `Some(μ.clamp(0.0, 1.0))` otherwise.

**D2 — μ-health gate (fail-closed).** `evaluate_mu_health(mu: Option<f64>, last_trade_ms: Option<u64>, now_ms: u64, cfg: &MuHealthConfig) -> Option<MuHealthReason>`; `None` = healthy (quoting/go-live permitted), `Some(reason)` = blocked. Order of checks (first failure wins):
1. `last_trade_ms` is `None` → `Absent` (no data ever).
2. `now_ms.saturating_sub(last_trade_ms) > cfg.stale_window_ms` → `Stale`.
3. `mu` is `None` → `Absent` (data present but no producible μ).
4. `mu` is non-finite → `NotFinite`.
5. `mu < cfg.mu_min_floor` → `BelowFloor` (degenerate / constant-0; spec §15 prohibits μ=0 go-live because GM collapses to zero spread).
6. else → `None` (healthy).
`MuHealthReason` = `enum { Absent, Stale, NotFinite, BelowFloor }`.

**D3 — placement.** Pure math (D1+D2 + the two config view-structs + `MuHealthReason`) → new `src/bolt_v3_maker_mu_estimator.rs`, ungated shared (mirrors `bolt_v3_trade_flow.rs` precedent), reads `crate::bolt_v3_trade_flow::{SignedTrade, SignedTradeFlow}` + `nautilus_model::enums::AggressorSide`, NO `crate::strategies` import. Per-strategy runtime state + wiring → new `src/strategies/binary_oracle_maker/mu.rs`, under MAKER_KEY (seal rotates).

**D4 — config contract (fail-closed go-live).** μ knobs are fields on `BinaryOracleMakerConfig` (taker-pattern; NOT a `[parameters]` table — confirm taker mapping in Task 4): `signed_flow_enabled: bool`, `trade_flow_window_secs: u64`, `trade_flow_max_samples: u64`, `mu_min_classified_samples: u64`, `mu_stale_window_ms: u64`, `mu_min_floor: f64`. `validate_config` enforces the go-live contract: reject if `signed_flow_enabled == false` ("μ source disabled — go-live prohibited, spec §15"), reject zero/degenerate knobs (`trade_flow_window_secs == 0`, `trade_flow_max_samples == 0`, `mu_min_classified_samples == 0`), reject `mu_min_floor` not in `(0.0, 1.0)` or non-finite, reject `mu_stale_window_ms == 0`. This validation is the structural go-live gate (μ "absent or constant-0" prohibited). Verify in Task 4 that `validate_config` runs in the node startup validation chain (R4: `validate_strategies → validate_strategy_archetype_with_bindings`); if the chain calls `StrategyBuilder::validate_config`, this suffices; otherwise also contribute the check from `archetype.rs::validate_strategy`. Prove with a test that invokes the actual go-live validation entry point.

**D5 — staleness source.** `last_trade_ms` for a buffer = `flow.samples().back().map(|s| s.ts_ms)` (most recent retained trade). No change to `bolt_v3_trade_flow.rs`.

**D6 — `on_trade` feeding.** The maker's `impl DataActor` gains `on_trade(&mut self, trade: &TradeTick) -> anyhow::Result<()>` that routes to `self.mu.observe(trade)` when `signed_flow_enabled`. No orders emitted. Buffers are created lazily per `InstrumentId` from the projected `SignedTradeFlowConfig`.

---

## File structure

- **Create** `src/bolt_v3_maker_mu_estimator.rs` — pure μ estimator + health gate + config views + `MuHealthReason` + unit tests.
- **Create** `src/strategies/binary_oracle_maker/mu.rs` — `MakerMuState` (per-instrument buffers + last-trade tracking), `observe`, `mu_for`, `health_for`, config projectors + unit tests.
- **Modify** `src/lib.rs` — add `pub mod bolt_v3_maker_mu_estimator;`.
- **Modify** `src/strategies/binary_oracle_maker/config.rs` — add 6 μ fields to `BinaryOracleMakerConfig`, extend the unknown-key whitelist, add range/enable validation.
- **Modify** `src/strategies/binary_oracle_maker/mod.rs` — declare `mod mu;`, add `mu: MakerMuState` field, build it in `new`, add `on_trade` to the `DataActor` impl.
- **Modify (verify)** `src/strategies/binary_oracle_maker/archetype.rs` — only if Task 4 proves `validate_config` is not on the go-live path.
- **Modify** `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml` — `[[allowed]]` rows for every new string literal (field names, validation messages, reasons).
- **Modify (re-record)** `src/bolt_v3_source_integrity.rs` — `GOLDEN_MAKER_DIGEST` from the CI value-stability surface (Task 7, last).
- **Modify** `config/strategies/` — NO new deployed operator TOML in Slice 2 (maker not deployed until Slice 9/10). Config is exercised via inline test fixtures only.

---

## Task 1: μ estimator pure function (`estimate_informed_fraction`)

**Files:** Create `src/bolt_v3_maker_mu_estimator.rs`; Modify `src/lib.rs`.

- [ ] **Step 1: Declare the module.** Add `pub mod bolt_v3_maker_mu_estimator;` to `src/lib.rs` immediately after the `pub mod bolt_v3_trade_flow;` line (`:38`), preserving alphabetical-ish grouping with the other `bolt_v3_maker_*` decls if present.

- [ ] **Step 2: Write the failing test module** in `bolt_v3_maker_mu_estimator.rs` with `MuEstimatorConfig { pub min_classified_samples: u64 }` and these `estimate_informed_fraction(&SignedTradeFlow, now_ms, &MuEstimatorConfig) -> Option<f64>` cases. Build `SignedTradeFlow` via `SignedTradeFlow::from_config` + `observe` with constructed `TradeTick`s (copy the `trade_tick_with_aggressor` test helper pattern from `bolt_v3_trade_flow.rs:198-235`; all magnitudes via named `const`s, no bare literals):
  - balanced flow (equal buy/sell volume, ≥ min samples) → `Some(0.0)`.
  - fully one-sided (all Buyer) → `Some(1.0)`.
  - 75/25 buy/sell volume split → `Some(0.5)` (|3−1|/4).
  - classified count below `min_classified_samples` → `None`.
  - only `NoAggressor` trades (count ≥ min by raw count) → `None` (unclassified excluded → zero classified).
  - mixed incl. `NoAggressor` → `NoAggressor` excluded from both sums and the classified count.
  - all samples aged out of `now_ms` window → `None`.
  - result clamped into [0,1] and always finite when `Some`.

- [ ] **Step 3: Run tests, verify they fail** (function not defined): local `cargo +1.95.0 build` is REFUSED — use `git push` and adjudicate advisory CI, OR (for faster local signal) run only `cargo +1.95.0 fmt --check`. Per repo policy, the failing-test gate is observed via CI on push; mark this step satisfied by the red CI run for the new test commit. (Same for all "verify fails/passes" steps below — fast local gates are fmt + `just source-fence`; full test is CI.)

- [ ] **Step 4: Implement `estimate_informed_fraction`** per D1 — iterate `flow.samples_within(now_ms)`, match `s.aggressor` (`AggressorSide::Buyer` → `buy_vol += s.size; classified += 1`; `Seller` → `sell_vol += s.size; classified += 1`; `NoAggressor` → skip); guard `classified < cfg.min_classified_samples` → `None`; `total = buy_vol + sell_vol`; guard `!(total > 0.0)` → `None`; `mu = (buy_vol - sell_vol).abs() / total`; guard `!mu.is_finite()` → `None`; else `Some(mu.clamp(0.0, 1.0))`. Module doc comment: states this is the net-new VPIN-style informed-fraction estimator (NT lacks it), names `gm_binary_quote` as the consumer, and the fail-closed contract.

- [ ] **Step 5: Verify tests pass (CI green for the new test).**

- [ ] **Step 6: Commit** `feat(488): net-new μ informed-fraction estimator over signed trade flow (Slice 2)`.

## Task 2: μ-health gate pure function (`evaluate_mu_health`)

**Files:** Modify `src/bolt_v3_maker_mu_estimator.rs`.

- [ ] **Step 1: Failing tests** for `MuHealthConfig { pub stale_window_ms: u64, pub mu_min_floor: f64 }`, `enum MuHealthReason { Absent, Stale, NotFinite, BelowFloor }` (`#[derive(Debug, Clone, Copy, PartialEq, Eq)]`), and `evaluate_mu_health(mu: Option<f64>, last_trade_ms: Option<u64>, now_ms: u64, &MuHealthConfig) -> Option<MuHealthReason>` per D2:
  - `last_trade_ms = None` → `Some(Absent)` (even if `mu = Some(healthy)`).
  - fresh data + `mu = None` → `Some(Absent)`.
  - `now - last_trade_ms` exactly `== stale_window_ms` → healthy boundary (NOT stale); `> stale_window_ms` → `Some(Stale)`.
  - `mu = Some(NaN)` (fresh) → `Some(NotFinite)`.
  - `mu = Some(0.0)`, `mu_min_floor > 0` → `Some(BelowFloor)`.
  - `mu = Some(floor)` exactly → healthy (`>= floor`); `mu = Some(value > floor)` → `None`.
  - stale takes precedence over a below-floor μ (check order).

- [ ] **Step 2: Verify fail. Step 3: Implement** per D2 exact order (note `MuHealthReason` is `Eq`-derivable only because it carries no `f64` — keep it fieldless). **Step 4: Verify pass. Step 5: Commit** `feat(488): fail-closed μ-health gate (absent/stale/non-finite/below-floor) (Slice 2)`.

## Task 3: per-instrument maker μ runtime state (`MakerMuState`)

**Files:** Create `src/strategies/binary_oracle_maker/mu.rs`.

- [ ] **Step 1: Failing tests** for `MakerMuState` with:
  - `new(estimator: MuEstimatorConfig, health: MuHealthConfig, flow: SignedTradeFlowConfig) -> Self` (plain owned config views; no TOML here).
  - `observe(&mut self, trade: &TradeTick)` — lazily creates a `SignedTradeFlow` per `trade.instrument_id` from the stored `SignedTradeFlowConfig`, calls `flow.observe(trade)`, and records `last_trade_ms` per instrument as `max(prev, trade.ts_event ms)`.
  - `mu_for(&self, instrument_id: &InstrumentId, now_ms: u64) -> Option<f64>` — delegates to `estimate_informed_fraction`.
  - `health_for(&self, instrument_id: &InstrumentId, now_ms: u64) -> Option<MuHealthReason>` — derives `last_trade_ms` from `flow.samples().back()` (D5) OR the tracked value (use one, document it), calls `evaluate_mu_health(self.mu_for(...), last_trade_ms, now_ms, &self.health)`. For an unknown instrument → `Some(Absent)` (fail-closed).
  - Test cases: unknown instrument → `health_for` = `Absent`; after observing a healthy one-sided burst → `mu_for` Some, `health_for` None; after observing balanced flow → `health_for` = `BelowFloor`; two instruments tracked independently.

- [ ] **Step 2: Verify fail. Step 3: Implement** `MakerMuState { estimator: MuEstimatorConfig, health: MuHealthConfig, flow_config: SignedTradeFlowConfig, flows: BTreeMap<InstrumentId, SignedTradeFlow> }`. Imports: `crate::bolt_v3_maker_mu_estimator::{...}`, `crate::bolt_v3_trade_flow::{SignedTradeFlow, SignedTradeFlowConfig}`, `nautilus_model::{data::TradeTick, identifiers::InstrumentId}`. (This file is under `strategies/` so importing `crate::bolt_v3_*` is the allowed direction.) **Step 4: Verify pass. Step 5: Commit** `feat(488): per-instrument maker μ runtime state + health (Slice 2)`.

## Task 4: maker config μ knobs + fail-closed go-live validation

**Files:** Modify `src/strategies/binary_oracle_maker/config.rs`; verify `archetype.rs` / go-live chain.

- [ ] **Step 1: Confirm the taker config→TOML mapping.** Read `src/strategies/binary_oracle_edge_taker/config.rs:60-90` and a deployed `config/strategies/binary_oracle_btc.toml` to confirm trade-flow knobs are flat config-struct fields (not a `[parameters]` table). If confirmed, μ knobs follow the same shape and `archetype.rs`'s `[parameters]` rejection stays. Record the finding in the commit body.

- [ ] **Step 2: Failing tests** in `config.rs` tests for:
  - a full valid maker config (3 base fields + 6 μ fields) parses + validates clean.
  - `signed_flow_enabled = false` → validation error "μ source disabled — go-live prohibited" (the structural go-live gate).
  - each of `trade_flow_window_secs = 0`, `trade_flow_max_samples = 0`, `mu_min_classified_samples = 0`, `mu_stale_window_ms = 0` → distinct validation error.
  - `mu_min_floor` ∈ {`0.0`, `1.0`, `-0.1`, `1.1`, NaN} → validation error; `mu_min_floor = 0.05` (in `(0,1)`) → clean.
  - an unknown extra field → still rejected (deny_unknown_fields preserved).

- [ ] **Step 3: Verify fail. Step 4: Implement** — add the 6 `pub` fields to `BinaryOracleMakerConfig`; extend the unknown-key whitelist loop (config.rs `:71-82` region) with the 6 new names; add a `validate_mu_parameters` helper called from `validate_config` enforcing D4 (all messages via named `const`s; ranges fail-closed). Keep `#[serde(deny_unknown_fields)]`.

- [ ] **Step 5: Prove go-live blocking.** Add a test that drives the *actual* startup validation entry point identified in Step 1 (e.g. `validate_strategies` or the archetype `validate_strategy` binding) with a `signed_flow_enabled=false` maker config and asserts it is rejected. If `validate_config` is not on that chain, additionally wire the enable/range check into `archetype.rs::validate_strategy` and re-test. **Step 6: Verify pass. Step 7: Commit** `fix(488): maker μ config knobs + fail-closed go-live gate (Slice 2)`.

## Task 5: wire `on_trade` into the maker

**Files:** Modify `src/strategies/binary_oracle_maker/mod.rs`.

- [ ] **Step 1: Failing test** that constructs a `BinaryOracleMaker` from a valid μ config, feeds two `TradeTick`s for one instrument via the maker's trade path, and asserts the maker's μ/health for that instrument transitions from `Absent` to a computed state. (Expose a `#[cfg(test)]`-visible accessor or a small `pub(crate) fn mu_health_for` on the maker if the `DataActor` `on_trade` cannot be invoked directly in a unit test; prefer driving `self.mu.observe` through a thin `pub(crate)` method that `on_trade` also calls — single code path, no test-only logic.)

- [ ] **Step 2: Verify fail. Step 3: Implement** — `mod mu;` decl; add `mu: MakerMuState` to the struct; build it in `new` by projecting the config's μ knobs into the three config views (only when `signed_flow_enabled`; else build an estimator that always reports `Absent` — but since `validate_config` rejects `enabled=false` at go-live, the runtime always has it enabled; still construct defensively). Add to the `impl DataActor for BinaryOracleMaker`:
  ```
  fn on_trade(&mut self, trade: &TradeTick) -> anyhow::Result<()> {
      self.observe_trade(trade);
      Ok(())
  }
  ```
  where `observe_trade` is the single `pub(crate)` routing method (`self.mu.observe(trade)`), so production and tests share one path. The impl is no longer empty — update the inert-guarantee doc comment to "observes trades to estimate μ and exposes a health gate; emits no orders (quoting is Slice 6)."

- [ ] **Step 4: Verify pass. Step 5: Commit** `feat(488): maker observes trades → μ estimate + health (no orders) (Slice 2)`.

## Task 6: runtime-literal audit rows

**Files:** Modify `docs/bolt-v3/research/runtime-literals/bolt-v3-runtime-literal-audit.toml`.

- [ ] **Step 1.** For every new string literal introduced in Tasks 1–5 (config field-name constants, validation-message constants, any reason text), add an `[[allowed]]` row mirroring the existing maker rows' classification + reason style (the current maker rows end ~`:9699-9802`). **Step 2.** Push; the `verify_bolt_v3_runtime_literals.py` CI fence must pass (every literal classified). **Step 3: Commit** `chore(488): classify Slice 2 μ-config runtime literals (audit)`.

## Task 7: re-record `GOLDEN_MAKER_DIGEST` (LAST)

**Files:** Modify `src/bolt_v3_source_integrity.rs`.

- [ ] **Step 1.** The maker-dir changes (config.rs, mod.rs, mu.rs) rotated the MAKER_KEY digest; the `value_stability_maker_digest_equals_golden_constant` test now fails loud with the new `left` value on CI. Read the surfaced value from the CI test output. **Step 2.** Update `GOLDEN_MAKER_DIGEST` to that value, with a `// Re-derived: Slice 2 added μ estimator state/config to the maker dir.` provenance comment. **Step 3.** Push; confirm the value-stability + all seal tests pass on CI. **Step 4: Commit** with a HEX-FREE message: `fix(488): re-record maker source-integrity seal after Slice 2 μ subsystem`.

---

## Definition of done (Slice 2)

- μ estimator + health gate are pure, fully unit-tested, fail-closed (absent/stale/non-finite/below-floor all block).
- The maker observes trades → per-instrument μ + health; emits no orders.
- A maker config with μ disabled or out-of-range knobs is rejected at the go-live validation entry point (proven by a test on the real chain).
- All CI fences green: fmt, clippy, dependency-direction (estimator has no `crate::strategies` ref), legacy-default, runtime-literals, source-integrity seal (re-recorded), full test suite.
- Committed on `feat/488-generic-maker` (single PR #716). Slice 2 then goes through Codex adversarial + internal adversarial review; every finding FIXED or DISPROVEN before Slice 3.

## Self-review (done at plan-write time)

- **Spec coverage:** §16#6 (μ source + health gate blocking quoting AND go-live) — Tasks 1,2,4,5. FR-021 (subscribe + classify) — classifier reused (`SignedTradeFlow`), estimator net-new (Task 1), feeding seam (Task 5); live subscription explicitly deferred to Slice 6/9 (stated). μ=0 prohibition (§15) — Tasks 2 (BelowFloor) + 4 (go-live reject).
- **NT-first:** estimator/gate are genuine residue (NT has no production VPIN/μ); `SignedTradeFlow` + `AggressorSide` + `gm_binary_quote` reused, not rebuilt.
- **No-hardcode / no-dual-path:** all knobs from config; μ knobs follow the single taker config-field pattern (Task 4 confirms); one `observe_trade` path for prod+test.
- **Type consistency:** `MuEstimatorConfig`, `MuHealthConfig`, `MuHealthReason`, `estimate_informed_fraction`, `evaluate_mu_health`, `MakerMuState`, `observe`/`mu_for`/`health_for` used identically across tasks.
- **Seal discipline:** estimator ungated (no rotation); maker-dir change re-records `GOLDEN_MAKER_DIGEST` last (Task 7).
