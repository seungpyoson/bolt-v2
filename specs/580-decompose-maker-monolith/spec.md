# Feature Specification: Port + decompose the #488 binary-oracle maker onto shared agnostic `bolt_v3_*` helpers

Issue: #580 · Child of #488 (oracle-anchored binary maker umbrella) · Adopts the architecture of #522 (decompose bolt-v3 monoliths).
Baseline: `main` @ `da7247f0`. All structural claims below were re-verified at this HEAD.

## Problem

The #488 maker is ~85% built but stranded **off `main`** on 4 disjoint stale branches (150–450 commits behind). Three structural problems block landing it:

1. **Monolith in the wrong layer.** ~80% of `src/strategies/maker_*` and `portfolio_*` is pure, family-agnostic math whose only non-`std` import is `crate::bolt_v3_numeric` — i.e. shared-helper material sitting inside the strategy layer. Landing the branches as-is reproduces the exact taker monolith #522 is decomposing.
2. **A second family-dispatch mechanism.** The maker branch introduces `pub trait MakerFamily` + `&dyn MakerFamily` (`maker_quote.rs:186/319`). Verified at `da7247f0`: there is **no** `trait *Family` or `dyn *Family` anywhere in `src/` on `main` — every family / provider / archetype seam is a `const *_BINDINGS` fn-pointer table (`MarketFamilyValidationBinding`, `ProviderBinding`, `ArchetypeValidationBinding`). A trait-object family seam would be the **only** one in the codebase: two mechanisms on one axis = a NO-DUAL-PATHS violation.
3. **A re-implemented admission primitive.** `maker_reservation` re-implements `bolt_v3_submit_admission`'s notional/fee math (`base_quantity_admission_notional`, `fee_inclusive_admission_notional`) in `f64` instead of reusing it — a genuine dual path.

## Goal

Port the maker onto `main` so that, from the first slice:

- family-**blind** engine modules are **flat** `src/bolt_v3_*.rs` shared helpers (taker-parity);
- the binary-vs-perp **family adapter** folds into the canonical `MarketFamilyValidationBinding` as new fn-pointer fields (impls in `updown.rs`); the standalone `trait MakerFamily` is **dropped**;
- the `submit_admission` notional/fee primitive is **reused**, not re-implemented;
- the NT/TOML strategy shell (`binary_oracle_maker.rs` archetype) is written **last**, as thin Layer-3 orchestration over the shared helpers;
- every slice is **TDD** (RED → GREEN → move → verify), independently fence-green, and independently mergeable.

This is a **behavior-preserving port + relayout**, not new maker behavior.

## North Star (why each move is principled, not cosmetic)

- **The dependency fence must be green from day 1.** `verify_bolt_v3_dependency_direction.py` forbids `bolt_v3_*` from importing `crate::strategies::*`. Hoisting the agnostic math *before* writing the archetype means the fence never goes red, instead of decomposing a landed monolith later.
- **One family seam.** The fn-pointer binding table is the codebase's single family-dispatch mechanism. The maker's write-side (quote targets, settlement payout, binary fee-curve) *joins* it; it does not fork it. Deletion test: removing `trait MakerFamily` and folding its two stateless unit-struct impls into fn-pointer fields makes complexity vanish — it is a pass-through over the existing seam, not a load-bearing module.
- **Single source of truth for fair value.** The maker consumes `fair_probability_up_for_family`; it never re-derives the BS digital `N(d2)`.
- **Reuse over re-implement.** The `f64` reservation gate and the `Decimal` admission cap share one extracted notional/fee primitive; `Decimal` stays authoritative, `f64` is a thin derived adapter.

## Functional Requirements

- **FR-001 — PORT, not rebase.** Re-port module bodies onto fresh PRs against `main`. The 4 stale branches are read-only port sources, closed once drained. No stale branch is merged.
- **FR-002 — No prerequisite slice lands an unused symbol; every shared symbol travels with its first consumer.** The repo's `-D warnings` gate treats an unused `pub(crate)` item as `dead_code` and fails the build — so a standalone "add the constants first" slice is impossible (confirmed: a PR adding `TWO_F64`/`HALF_F64` alone fails clippy with `constant is never used`). Each new shared symbol (numeric constant, helper, adapter) is introduced in the **same slice as the first ported module that uses it**, exercised by that module's test. For the three symbols verified missing on `main` (`TWO_F64`, `HALF_F64`, `sanitize_open_probability`, each `pub(crate)`): `TWO_F64` lands with `maker_model` (its first user, per usage scan); `HALF_F64` and `sanitize_open_probability` land with the `compose_binary_legs`/`resolve_band` relocation (their first user). There is no separate numeric PR.
- **FR-003 — Flat engine helpers.** Each family-blind maker engine module lands as a flat `src/bolt_v3_*.rs` sibling (no `src/bolt_v3_maker/` directory), matching the flat taker pair. A `bolt_v3_<name>/` directory is created only if a module earns it under the repo's directory rule (a fn-pointer dispatch root over swappable variants, or a single concept splitting into ≥3 cohesive files).
- **FR-004 — Family adapter folds into the canonical binding.** The maker family write-side (`quote_targets`, settlement 0/1 payout, binary fee-curve `fee_rate·p·(1−p)`) is added to `MarketFamilyValidationBinding` as fn-pointer fields, with impls in `updown.rs` and registration in `VALIDATION_BINDINGS`. `trait MakerFamily` / `&dyn MakerFamily` does not exist on `main` after this work. The rebate **accrual/accumulator** (the `rebate_share` multiply + PnL booking) stays family-**blind** in a shared reward helper and *calls* the family curve.
- **FR-005 — Cycle broken by relocation, not trait-inversion.** The only true import cycle is `maker_offsets` ↔ `maker_quote` (each imports the other's free function: `resolve_band` is defined in `maker_quote`, imported by `maker_offsets`; `compose_binary_legs` is defined in `maker_offsets`, imported by `maker_quote`). **Both functions are family-agnostic scalar math** (verified: `compose_binary_legs` at `maker_offsets.rs:122` takes only `f64` inputs and calls only `resolve_band`/`time_widening_factor`/`reward_shaping_offset`; it never touches `quote_targets` or `MakerFamily`). The cycle is therefore a pure placement artifact: relocate both functions and their shared value types into one shared module, `bolt_v3_quoting`, which the strategy layer depends on and which depends on neither origin module. No trait, no dependency injection, and **no dependency on the family fold (FR-004)** — the relocation is purely agnostic and can land before the fold.
- **FR-006 — Admission dual-path never lands.** The notional/fee math has exactly one home: `bolt_v3_submit_admission` already exposes it as the `Decimal`-authoritative pub fns `base_quantity_admission_notional` (`:627`) and `fee_inclusive_admission_notional` (`:747`), already used on `main`. When the reservation gate is ported, it **calls** that single primitive directly (converting its `f64` inputs to `Decimal` at the boundary); the stranded `f64` re-implementation (`maker_reservation.rs` `BuyCommitment::notional` + `gross_up`) is **not** ported. The reuse therefore happens *inside* the reservation-port slice itself — there is no separate earlier "reuse" slice (an `f64` adapter added ahead of its caller would itself be `dead_code`, per FR-002). An `f64` adapter is introduced only if a benchmark shows the boundary conversion is too costly, and only in the same slice as its caller, co-located with and derived from the `Decimal` fn with a parity test. The BPS-vs-multiplier fee representation is reconciled at the boundary (BPS → multiplier is `1 + bps/10000`).
- **FR-007 — Archetype last.** The maker archetype is written only after the shared helpers exist at `src/strategies/binary_oracle_maker/archetype.rs`, as a peer to the taker and complete-set strategy-owned archetypes. Generic validation remains in `src/bolt_v3_archetypes/mod.rs`, while production aggregation lives in `src/strategy_bindings.rs`; no production `src/bolt_v3_*` module imports `crate::strategies::*`.
- **FR-008 — TDD every slice.** RED (a failing test in the destination module) → GREEN (port the body until it passes) → move (delete from origin, repath imports) → verify. No body is ported without its test arriving first or alongside; no test is dropped in the move.
- **FR-009 — Fences green every slice.** The current `just source-fence-static-fences-only` lane passes on every slice, including naming, dependency-direction (empty/shrink-only allowance), core-boundary, provider-leak, runtime-literal, gated-source-root, and strategy-policy gates, plus `cargo fmt --check`, `just deny`, and the slice's targeted tests where compilation is authorized.
- **FR-010 — Single source of truth preserved.** Maker fair value comes from `fair_probability_up_for_family`; fees stay owned by `bolt_v3_providers/polymarket/fees.rs`; settlement is gross-of-fees. No duplicated state, config, or logic is introduced by the port.
- **FR-011 — The fold is stateless-by-design, and stays single-mechanism if a stateful family ever appears.** The folded fn-pointer fields dispatch to free functions; the current families (`BinaryFamily`, `LinearPerpFamily`) are stateless unit structs, so this is lossless. If a future family needs per-instance state, it is supplied by threading a state-carrying **input struct** through the binding fn (the pattern `fair_probability_up` already uses with `FairProbabilityInputs`) — **not** by adding a raw pointer field and **not** by reverting to a `trait`/`&dyn` seam. NO-DUAL-PATHS holds permanently: the fn-pointer binding table stays the one family-dispatch mechanism.

## Non-Goals

- New maker behavior, model changes, or tuning. This port preserves behavior.
- Wiring the archetype to a live venue / arming submit (that is #488 W-series go-live).
- Implementing IV, VPIN-derived μ, or the FR-023 graduated governor (tracked in #488 / #555).
- Merging or rebasing the stale maker branches.
- The reward layer's economic *policy* (W7) — only its agnostic-vs-family split is in scope.

## Acceptance

- Every maker engine module lives on `main` as a `bolt_v3_*` shared helper; the binary family write-side lives as fn-pointer fields on `MarketFamilyValidationBinding` with impls in `updown.rs`. No agnostic maker math remains homed in `src/strategies/`.
- `trait MakerFamily` / `&dyn MakerFamily` does not exist anywhere in `src/` on `main`.
- `bolt_v3_numeric` carries `TWO_F64`, `HALF_F64`, `sanitize_open_probability`, each tested.
- `submit_admission`'s notional/fee primitive has exactly one implementation, called by both the admission cap and the reservation gate; the stranded `f64` re-implementation never appears on `main` at any commit (its reuse slice precedes the reservation port).
- Every ported module's tests travel and pass; the full suite is green on `main` after each slice merges (CI authoritative).
- All 4 fence scripts pass on every slice.
- If the archetype is built in scope, it is pure Layer-3 and imports only `bolt_v3_*`.

## Per-Slice Gate (every slice, no exceptions)

```
RED    — port the destination module's test FIRST; confirm it fails for the right reason
GREEN  — port the body until the test passes AGAINST main's current API. The stale
         branches are 150–450 commits behind, so a ported test is validated against the
         current baseline, not assumed green from its origin branch.
MOVE   — delete the origin copy; repath every importer in the same commit
VERIFY — cargo fmt --check && clippy -D warnings && cargo test <targeted>
         && verify_bolt_v3_naming.py && verify_bolt_v3_dependency_direction.py
         && verify_bolt_v3_core_boundary.py && strategy-policy fence
PR     — one slice = one PR = one declared scope (CLAUDE.md #9). No slice may land a
         known dual-path, even temporarily. No slice may land an unused symbol — the
         -D warnings dead_code gate forbids it, so every new shared symbol ships in the
         same slice as its first consumer (FR-002).
```

## Method & Ledger

Each slice is one PR with a declared scope and its port source named (`module → destination → source branch @sha → PR`). The plan (`plan.md`) carries the port-source matrix, the target module layout, and the slice sequence. External-model approval (codex + glm + deepseek, ≥ gemini) is required on this spec and plan before the first code-moving PR.
