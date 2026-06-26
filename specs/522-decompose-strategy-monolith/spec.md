# Feature Specification: Decompose the bolt-v3 strategy + operator_artifacts monoliths

**Tracking**: #522 | **Date**: 2026-06-02 | **Base**: `origin/main` `2938bc6f`
**Findings**: `docs/bolt-v3/2026-06-02-monolith-findings.md`

## Problem

`src/strategies/binary_oracle_edge_taker.rs` (18,205 lines, 229 embedded tests) and
`src/bolt_v3_operator_artifacts.rs` (17,466 lines) together are ~45% of `src/`. The
strategy file owns concerns that, per
[AGENTS.md#repo-rule-strategies-produce-intent-only](../../AGENTS.md#repo-rule-strategies-produce-intent-only), must live in shared
execution/admission modules (admission-request construction, fee-adjusted sizing,
rounding/precision, submit gating) — and bundles config, market selection, pricing
state, decision math, exposure lifecycle, order construction, source-proof, the
`DataActor` event surface, and the full test suite in one file. This blocks
navigation, slows compilation feedback, entangles maker/taker reuse, and makes
behavior hard to reason about and debug.

## Goal

Decompose both monoliths into focused modules with **intended shared helpers**, so
each module does one job, the strategy file holds only intent + signal +
orchestration ([AGENTS.md#repo-rule-strategies-produce-intent-only](../../AGENTS.md#repo-rule-strategies-produce-intent-only)), and the #488 maker can reuse the taker's pure helpers
without copying. Every move is **behavior-preserving** (no logic change inside a
move slice) and verified.

## North Star (why each move is principled, not cosmetic)

1. **[AGENTS.md#repo-rule-strategies-produce-intent-only](../../AGENTS.md#repo-rule-strategies-produce-intent-only)** — strategies emit intent/signal only; admission, sizing,
   rounding, fee-adjustment, submit gating belong in shared modules.
2. **spec 023 (NT order-intent layer)** — the shared landing zone already exists
   (`bolt_v3_order_intent`, `bolt_v3_submit_admission`); the taker is retrofit onto it.
3. **#488 maker reuse** — extracting the taker's pure decision/sizing/pricing helpers
   as shared modules directly serves the maker, which is a new archetype that reuses
   them.

## Functional Requirements

- **FR-001** Every slice is behavior-preserving: a move slice changes module location
  and visibility/re-export only; it MUST NOT change runtime logic, numeric behavior,
  or public test outcomes.
- **FR-002** Each slice records **RED/GREEN characterization** before code movement:
  a test that pins the current behavior of the moved unit, shown failing against a
  deliberately-wrong control and passing after the move (or, for a pure relocation,
  the relocated tests run green at both the pre-move and post-move HEAD).
- **FR-003** Extraction MUST keep every existing call site and test file green, with
  the mechanism chosen by caller scope:
  - Units with callers **outside** the origin module (e.g. `operator_artifacts`'s public
    API consumed by `tests/bolt_v3_operator_artifacts.rs`): preserve the path via
    **re-export** (`pub use`) from the origin — the proven #520 pattern.
  - Units **private** to the origin module (no external callers, e.g. A1's signal math):
    relocate and update the origin's in-file `use` imports; do **NOT** add a `pub use`
    (a needless crate-visible surface is itself a dual surface).
  No caller or test file outside the slice's declared scope changes.
- **FR-004** **No dual paths**: a decomposition slice MUST NOT introduce a second way
  to do a thing. Extraction consolidates; it never forks. Any test-only duplicate of
  production logic (e.g. the duplicate admission builder at `binary_oracle_edge_taker.rs:7546`)
  is removed in favor of the single shared unit.
- **FR-005** **No hardcodes introduced**: moved code carries its existing
  config-derived values; no new string/number literals for runtime values.
- **FR-006** Shared modules stay **strategy- and venue-agnostic** at their boundary
  (spec 023 contract): a shared helper takes typed inputs and returns typed outputs;
  it does not import strategy archetypes, provider names, evidence, or admission policy
  unless that is the module's declared job.
- **FR-007** One declared scope per PR (`AGENTS.md`). Each slice PR names the ledger
  items it resolves and the accepted scope that remains.
- **FR-008** Public API of `operator_artifacts` is preserved via re-export so
  `tests/bolt_v3_operator_artifacts.rs` (touched by in-flight #507/#510) stays green.
- **FR-009** `just source-fence` and the runtime-literal audit pass on every slice;
  the Spec Kit pointer in `AGENTS.md` is not repurposed.

## Non-Goals

- No behavior/economics change. (Logic changes are separate, separately-reviewed work.)
- No new strategy capability, no maker work (#488 owns that).
- No removal of the hand-rolled digital (that is #488 W8).
- No change to NT lifecycle, risk, execution, or adapter ownership.

## Acceptance

- Each slice: behavior-preserving move landed behind the per-slice gate (below);
  `binary_oracle_edge_taker.rs` / `operator_artifacts.rs` line count strictly
  decreases; the extracted module is independently testable.
- Epic #522 acceptance = all Track A + Track B ledger items resolved, the strategy
  file holds only struct + `DataActor` orchestration + intent/signal glue, and the
  shared helpers are consumed (not copied) by both archetypes.

## Per-Slice Gate (every slice, no exceptions)

speckit plan → adversarial review (`/codex:adversarial-review` + relay:
grok/glm/deepseek/kimi/gemini) → `/tdd` implementation via fan-out → PR → exact-head
CI green → unanimous 6-model approval → **explicit operator merge permission**. No PR
merges without it. A model that does not respond after 2 attempts is waived and the
waiver recorded (precedent: Kimi waivers on #470/#474/#479).

## Method & Ledger

Precedent #454/#466: evidence-ledger-first, narrow extraction (proven-equivalent
only), multi-PR slices, epic stays open until all items resolved + operator approval.
Ledger lives in `evidence.md`.
