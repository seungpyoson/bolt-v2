# Implementation Plan: Port + decompose the #488 maker

Issue: #580 · Spec: `specs/580-decompose-maker-monolith/spec.md` · Baseline: `main` @ `da7247f0`.

## Summary

Port the stranded #488 maker onto `main` as shared agnostic `bolt_v3_*` helpers plus a folded family binding, in TDD slices that each keep the dependency fence green. Engine modules land flat; the binary family write-side folds into `MarketFamilyValidationBinding`; the `submit_admission` primitive is reused; the archetype is written last. Behavior-preserving.

## Technical Context

- **Language / runtime:** pure Rust, single crate, NautilusTrader Rust API. The maker modules are NT-free pure logic (sole non-`std` import = `crate::bolt_v3_numeric` + sibling `crate::strategies::*`), which makes them clean port inputs.
- **Fences (must pass every slice):**
  - `scripts/verify_bolt_v3_naming.py` — recursive `src/**/*.rs`, name-based.
  - `scripts/verify_bolt_v3_dependency_direction.py` — `src/bolt_v3_` prefix; forbids `crate::strategies::*`; `FINDING_ALLOWANCES` is shrink-only.
  - `scripts/verify_bolt_v3_core_boundary.py` — literal `CHECKS` list (the three `*/mod.rs` dispatch roots).
  - strategy-policy fence + `cargo fmt --check` + `clippy -D warnings`.
- **Verified preconditions @ `da7247f0`:** `bolt_v3_numeric` is missing `TWO_F64`/`HALF_F64`/`sanitize_open_probability`; no `trait/dyn *Family` in `src/`; `MarketFamilyValidationBinding` (mod.rs:58) carries `fair_probability_up` (:83), `VALIDATION_BINDINGS` (:271), `fair_probability_up_for_family` (:409); zero `maker_*` modules on `main`.

## Target Module Layout (end state, not one PR)

```
src/
  bolt_v3_numeric.rs                  # + TWO_F64, HALF_F64, sanitize_open_probability (PR-0)
  bolt_v3_quote_lifecycle.rs          # quote_lifecycle (+ LegEvent shared with maker_event_fence)
  bolt_v3_requote_budget.rs           # requote_budget (w4 cost-weighted variant)
  bolt_v3_maker_model.rs              # gm_binary_quote / gm_half_spread / inventory_skew
  bolt_v3_quoting.rs                  # microprice + resolve_band + compose_binary_legs + value types
                                      #   + time_widening + reward_shaping  (cycle absorbed here)
  bolt_v3_maker_governor.rs           # graduated governor state machine
  bolt_v3_maker_inventory.rs          # inventory accumulator
  bolt_v3_maker_reservation.rs        # reservation gate (calls extracted submit_admission primitive)
  bolt_v3_portfolio_selection.rs      # MarketKey newtype + selection
  bolt_v3_portfolio_allocator.rs      # allocator
  bolt_v3_portfolio_risk.rs           # risk
  bolt_v3_maker_rewards.rs            # rebate accrual + phantom_lp + shaper LEDGER (family-blind)
  bolt_v3_submit_admission.rs         # notional/fee primitive reused by reservation gate (PR-1)
  bolt_v3_market_families/
    mod.rs                            # MarketFamilyValidationBinding += quote_targets / settle / fee_curve
    updown.rs                         # impls of the new fn-pointer fields (binary family)
  bolt_v3_archetypes/
    binary_oracle_maker.rs            # NEW, LAST — Layer-3 NT/TOML shell, imports only bolt_v3_*
  strategies/
    maker_config.rs                   # ValidatedMakerConfig TOML schema (intent binding — stays)
    maker_resync.rs                   # reconnect / cancel-all-on-kill (maker intent alphabet — stays)
```

Notes:
- No `src/bolt_v3_maker/` directory — engine helpers are flat, matching `bolt_v3_taker_signal.rs` / `bolt_v3_taker_pricing.rs`.
- `OutcomeSide` stays `pub(crate)` in `market_families` (already crate-wide; the flat taker imports it). The maker keeps its own `QuoteSide` and takes `fair` as `f64`, so the fold is a design choice executed at wiring time, not a compile-forced move.

## Slice Sequencing

Each slice = one PR, independently fence-green and mergeable. Re-sequenced after external review so that **no known dual-path lands on `main` even temporarily**: the admission-primitive reuse (formerly last) now precedes the reservation port, and the coarse "numeric leaves" slice is split per-module.

- **PR-0 — numeric prerequisite.** Add `TWO_F64`, `HALF_F64`, `sanitize_open_probability` to `bolt_v3_numeric` (verbatim from `w4-settlement`), `pub(crate)`, with tests. Unblocks every slice that imports them. No maker code.
- **PR-1 — admission-primitive reuse (moved up; pure `main` refactor, no maker code).** `bolt_v3_submit_admission` already exposes `base_quantity_admission_notional` (`:627`) and `fee_inclusive_admission_notional` (`:747`) as `Decimal`-authoritative pub fns. **Default: documentation-only** — establish these as the single notional/fee primitive and have the reservation gate (PR-3d) call them directly, converting `f64`→`Decimal` at the boundary. **Only if a benchmark shows the boundary conversion is too costly in the requote loop** does this slice add an `f64` adapter, co-located with and documented as derived from the `Decimal` fn (BPS→multiplier = `1 + bps/10000`), with a parity test pinning `f64` ≈ `Decimal`. No speculative dead-code adapter. This slice must merge **before PR-3d** so the reservation gate never re-implements the math.
- **PR-2 — lifecycle.** `quote_lifecycle → bolt_v3_quote_lifecycle` (lift-verbatim; ~40 tests travel). Repath `maker_event_fence`'s `LegEvent` in the same commit.
- **PR-3 — numeric leaves, SPLIT per module (each independent, any order, each its own PR):**
  - **3a** `maker_model → bolt_v3_maker_model` (gm_binary_quote / gm_half_spread / inventory_skew).
  - **3b** `maker_microprice → bolt_v3_quoting` (seeds the shared quoting module that PR-4 completes). Verified independent of the cycle functions — `maker_microprice.rs` imports only `bolt_v3_numeric`, so it lands cleanly before PR-4.
  - **3c** `requote_budget → bolt_v3_requote_budget` — port the cost-weighted **w4** variant; explicitly reject and do not port the count-based **w2** variant (state the reason in the PR body).
  - **3d** `maker_reservation → bolt_v3_maker_reservation` — calls PR-1's primitive; the `f64` `gross_up`/`notional` re-impl is **not** ported. Depends on PR-1.
  - **3e** `maker_governor → bolt_v3_maker_governor`.
  - **3f** `portfolio_selection → bolt_v3_portfolio_selection` (carries the `MarketKey` newtype).
  - **3g** `maker_reward_{shaper,phantom_lp} → bolt_v3_maker_rewards` (family-blind ledger).
- **PR-4 — cycle-breaker (agnostic relocation, linchpin).** The only true cycle is `maker_offsets:resolve_band ↔ maker_quote:compose_binary_legs`, and **both functions are agnostic scalar math** (verified: `compose_binary_legs` at `maker_offsets.rs:122` is `f64`-only and never calls `quote_targets`/`MakerFamily`). Relocate `resolve_band` + the shared value types (`QuoteSide`, `QuoteTargetLeg`, `QuoteTargets`, `FamilyQuoteInputs`) + `compose_binary_legs` + `time_widening_factor` + `reward_shaping_offset` into the shared `bolt_v3_quoting` module (seeded by 3b). The cycle vanishes because `bolt_v3_quoting` depends on neither origin module and the strategy layer depends on it. **No trait, no fn injection, no dependency on PR-5** — this is purely agnostic. Depends on PR-3b. **Pre-flight gate:** before relocating, re-grep `compose_binary_legs` and `resolve_band` for any `MakerFamily`/`quote_targets` reference (currently zero at `maker_offsets.rs:122` / `maker_quote.rs:138`); if either ever gains a family reference, this slice instead injects the family fn as a parameter — but as verified today, it does not.
- **PR-5 — family seam (fold).** Add `quote_targets`, settlement payout, and the binary fee-curve `fee_rate·p·(1−p)` (real on `w5-w7` `maker_reward_rebate.rs:86`) as fn-pointer fields on `MarketFamilyValidationBinding`; impl in `updown.rs`; register in `VALIDATION_BINDINGS`; add dispatchers mirroring `fair_probability_up_for_family`. Drop `trait MakerFamily`. The binary impl binds `fair_probability_up_for_family` (never re-derives `N(d2)`). The rebate **accrual** stays family-blind in `bolt_v3_maker_rewards` and calls the family curve. Depends on PR-4.
- **PR-6 — strategy-resident consumers (repath-only).** Repath importers of the hoisted modules (`maker_inventory`, `maker_resync`, `maker_settlement`'s caller, `maker_config`, `maker_maintenance`, `maker_stale_quote`) in lockstep. Each can later hoist or stay — per-module call.
- **PR-7 — portfolio.** `portfolio_allocator + portfolio_risk → bolt_v3_portfolio_{allocator,risk}` (repath `portfolio_selection::*` after PR-3f).
- **Archetype — last.** Write `bolt_v3_archetypes/binary_oracle_maker.rs` once PR-1..PR-5 exist. The maker keeps its own `QuoteSide` while `market_families` owns `OutcomeSide`; the archetype is where the two are mapped, so that mapping is explicit and unit-tested here (not implicit).

## Port-Source Matrix

| Module(s) | Destination | Source branch @sha |
|---|---|---|
| `quote_lifecycle`, `maker_event_fence`, `requote_budget`, `maker_model`, `maker_microprice`, `maker_offsets`, `maker_quote`, `maker_governor`, `maker_inventory`, `maker_settlement` | per layout above | `feat/488-w4-settlement` @ `2dee3ed3` (W2+W3+gov+settlement superset) |
| `maker_config`, `maker_maintenance`, `maker_stale_quote` | `strategies/` (stay) / `bolt_v3_maker_ops` | `feat/488-maker-ops-readiness` @ `035d940b` (only source) |
| `maker_reservation`, `portfolio_{selection,allocator,risk}`, `maker_reward_{shaper,phantom_lp,rebate}` | per layout above | `feat/488-w5-w7-primitives` @ `96e10603` (only source) |
| — (DEAD) | — | `reference/488-helper-boundaries-mixed` @ `8602af8c` — added zero `bolt_v3_*`; its diff reverses A-series. Mine only its `architecture.md` migration table; never base on it. |

## Per-Slice Method (RED → GREEN → move → verify)

1. **RED** — port the destination module's test first; run it and confirm it fails for the right reason (missing symbol / unported body), not a compile error elsewhere.
2. **GREEN** — port the body; adapt only imports (`crate::bolt_v3_numeric`, sibling shared helpers); make the test pass **against `main`'s current API**. Because the source branches are 150–450 commits behind, do not assume a ported test is green as-is — reconcile it with the current baseline (signatures, renamed symbols) before it counts as GREEN.
3. **MOVE** — delete the origin copy; repath every importer in the same commit; demote rustdoc intra-doc links that would cross the fence to plain comments.
4. **VERIFY** — `cargo fmt --check` + `clippy -D warnings` + targeted `cargo test` + all 4 fences. Push; CI runs the full suite (do not block on a local full run).

## Rebase / In-flight Interaction

- This is decomp-spine work; it merges into `main` and the stale maker branches are never rebased onto it (they are drained, then closed).
- Coordinate `bolt_v3_numeric.rs` and `strategies/mod.rs` edits with any concurrent maker-generation PRs (#514/#515) — serialize touches to those shared files.
- Close as superseded once drained: `w2-quote-lifecycle`, `w3-governor`, `w4-settlement`, `maker-ops-readiness`, `w5-w7-primitives`, `reference/helper-boundaries-mixed`; PRs #514/#515 salvage-source-then-close (read diff + bidirectional link before close, per learnings #10/#11).

## Risks

- **Cycle mis-split (PR-4).** If value types are left behind, the back-edge stays cross-module. Mitigation: relocate the types *with* both (agnostic) functions in one slice; the dependency fence verifies no cross-module back-edge remains.
- **Legacy-test drift (all port slices).** A ported test may not be green against `main` because its origin branch is 150–450 commits behind (renamed symbols, changed signatures). Mitigation: the GREEN step reconciles each test against the current baseline; never assume origin-branch greenness.
- **Test loss in the move.** Mitigation: the per-slice gate forbids a body landing without its test; count tests before/after.
- **Fold scope creep (PR-5).** Only `quote_targets` + settlement + fee-curve fold; rebate accrual stays family-blind. Mitigation: spec FR-004 names the exact members.
- **Stateful family later (PR-5, deferred).** The fn-pointer fold is stateless-only. If a future family needs per-instance state, it is added by threading a state-carrying input struct through the binding fn (à la `FairProbabilityInputs`), never a raw pointer or a reverted trait (spec FR-011). No raw `c_void`/pointer field is added speculatively.
- **`requote_budget` variant collision.** Two variants exist (w2 count-based, w4 cost-weighted). Mitigation: port the w4 cost-weighted one (PR-3c); reject the other explicitly in the PR body.
- **Admission `f64`/`Decimal` parity (PR-1).** If an `f64` reservation face is added, it could drift from the `Decimal` authority. Mitigation: co-locate it, derive it from the `Decimal` fn, and pin a parity test; prefer calling the `Decimal` fn directly unless a benchmark forces the `f64` face.
- **Concurrent worktree churn.** Many active worktrees touch shared files (`bolt_v3_numeric.rs`, `strategies/mod.rs`). Mitigation: small slices, serialize shared-file edits, CI authoritative.

## Complexity Tracking

| Concern | Status |
|---|---|
| New shared modules created | ~11 (see layout) |
| Genuine dual-paths | 1 (`submit_admission` reservation re-impl) — prevented from ever landing: reuse slice PR-1 precedes the reservation port (PR-3d) |
| Second dispatch mechanism removed | 1 (`trait MakerFamily` → fn-pointer fields, PR-5) |
| Behavior change | none (port preserves behavior; CI + traveling tests prove it) |
