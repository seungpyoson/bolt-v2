# Implementation Plan: Positional Sizing Engine (#712)

**Branch**: `712-positional-sizing-engine` | **Date**: 2026-06-25 | **Spec**: `specs/712-positional-sizing-engine/712-positional-sizing-engine-spec.md`
**Input**: Feature specification from `specs/712-positional-sizing-engine/712-positional-sizing-engine-spec.md`. Tracking: #712. Depends on #711 and the risk-reservation substrate (`specs/973-risk-reservation-substrate/973-risk-reservation-substrate-spec.md`); armed live by #688.

## Summary

Build the real positional sizer: a selectable sizing model on one seam (fixed-fraction-of-equity — the calibration-free safe mode that launches; risk-constrained Kelly — the target growth model, gated on calibration #724) that sizes the **complete target terminal position** and emits the delta, behind one `SizingAdmissionCoordinator` that projects the target to an exact candidate via the substrate's shared evaluator, mints unforgeable provenance, submits to the substrate's atomic gate, and runs bounded retry + reduction. The core is family/venue/instrument-agnostic; binary/taker payoff lives only in a registered, sealed adapter that derives terminal cash flows from the active descriptor. The engine consumes a calibrated edge + coverage band and never measures calibration. It retires the fixed-notional `choose_robust_size`; there is no dual sizing path.

## Technical Context

**Language/Version**: Rust (edition per workspace).
**Primary Dependencies**: NautilusTrader Rust crates at the rev **pinned in `Cargo.toml`**. The risk-reservation substrate (this repo) owns the atomic gate, descriptor authority, advisory view, provenance verification, and SafetyAction path — #712 depends on it and re-implements none of it. Bankroll surface: `bolt_v3_sizing_state.rs::NtDerivedSizingState`.
**Storage**: None of its own; the substrate owns risk state + the reservation ledger. The engine is stateless between decisions except for reading the substrate's advisory view.
**Testing**: `cargo test` (unit + property), including a stateful-sizing test, an RCK optimization test (argmax log-growth + state-wise positivity + precondition), a target-scope-stress `ModelRiskCap` binding test (the cap, not the RCK constraint, sets the size; and `current_scope_stress` over the cap leaves only strict reductions, with no-trade distinct from a flat target), a no-silent-fallback test (Kelly attestation missing → no-trade unless an approved fixed-fraction fallback is in the epoch), a conservative-launch-cost test, a band-attestation no-trade test, and review/grep for the agnostic core + single cash-flow authority + single path; `cargo fmt`/`clippy`/`deny` clean. Evidence class per slice below; exact-head proof via `just verify-remote` before done. Off by default; live arming is #688 with fail-closed + exact-head evidence.
**Target Platform**: Linux (EC2 LiveNode) + offline tests.
**Project Type**: Single Rust project (NT thin-layer strategy sizing).
**Constraints**: NO HARDCODES (TOML, fail-closed), PURE RUST, NO DUAL PATHS (one sizing path; the safe model is a selectable model; one coordinator), SSM-only secrets, GROUP BY CHANGE, OFF BY DEFAULT. NO STRATEGY/VENUE/SYMBOL HARDCODING in the core.
**Scale/Scope**: Taker (P1) first; the harness is proven reusable via extension-seam conformance (P2). Maker sizing is a separate feature specification, not part of this engine.

## Constitution Check

*GATE: must pass before implementation; re-check after each slice.*

- **I. NT-First Thin Layer** — PASS. The engine is bolt's strategy decision policy + pre-submit sizing; it reserves through the substrate (which consumes NT truth) and adds no order-lifecycle/reconciliation machinery.
- **II. Generic Core, Concrete Edges** — PASS, and load-bearing here. The core (models, coordinator) is venue/family/strategy-agnostic; binary/taker structure lives only in a registered, sealed adapter selected by config (FR-030..FR-032; SC-005/006). A concrete venue/symbol/family branch in the core fails the gate.
- **III. Single Path And Config-Controlled Runtime** — PASS. The "safe" model is a selectable model, not a separate path; one coordinator owns target→action; each policy epoch selects one primary model with no automatic/silent fallback (FR-015); off by default; every parameter is TOML behind the substrate's policy envelope (SC-009).
- **IV. Evidence-Driven Verification Gates** — PASS. Off until #688; live requires fail-closed + exact-head proof. Kelly arms only behind the versioned `BandCoverageAttestation` defined by #724 FR-011, with a no-trade default and no silent fallback (FR-013/FR-015; SC-004/SC-011).
- **V. Evidence Before Claims** — PASS. Each slice maps to a named test or review/grep artifact at the exact head.
- **VI. Minimal Slice Discipline** — PASS. W0–W5 are independently shippable; each fails closed (zero-size / no-trade).
- **VII. Research/Analytics NT-First** — N/A; calibration measurement is explicitly out of scope (#724/#723), keeping this engine a single-job consumer of edge + band.

## Architecture — agnostic core, one coordinator, sealed adapter

- **Generic core**: `TargetPosition` + `SizingModel` seam (fixed-fraction-of-equity = calibration-free safe mode; RCK = target growth model gated on calibration #724). Models compute an allowance only (FR-002); the substrate enforces feasibility.
- **One coordinator**: `SizingAdmissionCoordinator` projects target→candidate via the shared evaluator, mints provenance, submits, runs bounded retry + reduction; risk-reducing closes route to the substrate SafetyAction path (FR-020..FR-022).
- **Sealed registered adapter**: binary/taker `RegisteredPayoffAdapter` owns S_model/probabilities and derives Πₛ from the active descriptor (FR-031); data-driven selection (FR-032). The harness is proven reusable by extension-seam conformance (FR-043); maker SIZING is a separate feature specification, not just another adapter.

## Project Structure

### Documentation (this feature)

```text
specs/712-positional-sizing-engine/
├── 712-positional-sizing-engine-spec.md   # the sizing spec
└── 712-positional-sizing-engine-plan.md   # this file
```

### Source Code (new sizing modules; retires choose_robust_size)

New sizing modules sit alongside the existing `bolt_v3_` strategy modules (exact paths confirmed at implementation; `position_sizer` name freed by #711). The agnostic core (target, model seam, coordinator) and the sealed binary/taker adapter are separate modules. The legacy `bolt_v3_sizing.rs::choose_robust_size` (fixed-notional × EV) is removed — no dual sizing path. Provenance/candidate/token contracts are imported from the substrate contracts module (single source of truth), not redefined.

**Structure Decision**: One agnostic sizing core + one coordinator + one sealed registered adapter; reuse the substrate's contracts and evaluator. Retire the fixed-notional sizer. The maker adapter (P2) plugs into the same harness.

## Workstreams (dependency-ordered; each fails closed; evidence class per slice)

- **W0 — Foundation & seams.** Retire `choose_robust_size`; define `TargetPosition`, `SizingIntent`, the `SizingModel` seam, and the sealed `RegisteredPayoffAdapter` trait; coordinator skeleton. Off by default. Depends on substrate S0–S2 contracts. *Evidence: review/grep (agnostic seam, single path, choose_robust_size removed) + `cargo test` skeleton; fmt/clippy/deny.*
- **W1 — Fixed-fraction-of-equity model (the calibration-free safe mode; launches first).** Allowance ρ·W (no headroom in the model); stateful target sizing (size the position, emit the delta). *Evidence: SC-001 (model consumes no headrooms) + SC-002 (delta-to-aggregate, not full-size) + SC-008 (size falls after drawdown).*
- **W2 — Coordinator.** Target→candidate projection via the shared evaluator; provenance minting (compile-time single authority); bounded retry + reduction protocol; risk-reducing close → substrate SafetyAction. Depends substrate S2/S4/S5. *Evidence: SC-007 (close not blocked by edge gate) + SC-009 (one path/one authority) + bounded-retry no-trade test + compile-fail test for forged candidate.*
- **W3 — Binary/taker sealed adapter + conservative cost.** S_model/probabilities; Πₛ derived from the active descriptor; fee/slippage all-in cost behind the curve-shaped interface, with the launch scalar REQUIRED to be a conservative upper bound on all-in unit outlay up to the candidate quantity under the admitted limit price (worst admitted price + every fee), and a post-rounding edge recheck (FR-040); zero-size on sub-edge/sub-min-lot/stale. *Evidence: SC-005 (Πₛ from descriptor; no second cash-flow source) + SC-006 (no family/venue/symbol branch in core) + SC-012 (conservative launch cost).*
- **W4 — RCK model (the target growth model; gated on calibration #724).** Solve x* ∈ argmax Σ πₛ·ln(Rₛ) subject to Σ πₛ·Rₛ^(−κ) ≤ 1, κ = ln β/ln α; zero-risk target feasible; state-wise strictly-positive Rₛ (the binding positivity rule; C(q)<W only a necessary screen, no NaN); S_model vs S_stress; side-aware adverse band end with Phase-1 long-only RCK arming (FR-012; short-side RCK rejected to no-trade until the deferred #724 FR-010B upper-band attestation). Arms only behind the canonical `BandCoverageAttestation` (#724 FR-011) consumed per FR-013 — decision-scoped fields exact-matched, evidence-scoped fields authenticated + accepted by the policy epoch, the whole artifact verified and its exact digest bound in the prepared epoch (#973 FR-050) — plus the concretely-specified `ModelRiskCap` (FR-014: absolute target-scope cap `cap_amount = min(absolute settlement-unit, dimensionless fraction·reference_growth_wealth)` bounding `target_scope_stress(x)` (current and post-target scoped stress substrate-computed via #973 FR-003/FR-025) over a substrate-recognized declared scope; over cap ⇒ strict reductions only; applied before the argmax, recomputed each decision) enforced independently. No-trade is distinct from a flat target, and existing exposure is never assumed to satisfy the constraint (FR-010). Model selection + fallback per FR-015: no automatic/silent fallback; default no-trade. **Depends on #724 being built.** *Evidence: SC-003 (post-target-wealth + precondition reject) + SC-004 (no attestation → no-trade) + SC-010 (argmax + zero-risk feasible + ModelRiskCap target-scope-stress binds + over-cap → strict reductions only) + SC-011 (no silent fallback).*
- **W5 — Extension-seam conformance (P2).** Register a second, trivial conformance adapter on the same coordinator/substrate/token path to PROVE the harness is reusable with only a payoff adapter swapped (FR-043). This slice does NOT build maker sizing — maker sizing (fill probability, queue, inventory, multi-order bundles) is a separate feature specification. *Evidence: review (same harness, adapter-only difference) + conformance-adapter unit test; maker sizing explicitly out of scope.*

Live arming (enforce on, exact-head proof) is out of scope here and tracked by #688.

## Complexity Tracking

| Decision | Why needed | Simpler alternative rejected because |
|----------|------------|--------------------------------------|
| Sealed registered adapter (not a typed binary input) | Keeps the core strategy/venue/symbol-agnostic per the user's hard constraint and Constitution II | A typed binary input (point/band probabilities, true/false payoffs) encodes binary structure in the core, blocking maker/future-family reuse |
| RCK on post-trade target wealth | Existing same-instrument exposure must bind the constraint or per-decision sizing returns | Order-delta P&L lets each slice look safe while the aggregate breaches the drawdown constraint |
| Worst-case-edge sizing without a fractional multiplier | Worst-case band end is already box-DRO at fraction 1; a separate ¼–½ multiplier double-counts the same risk | Re-adding fractional Kelly mis-budgets; band misspecification is a distinct risk guarded by attestation + a named model-risk cap |
| Extension-seam conformance, not a maker build | The harness must be provably reusable, but maker sizing (fill prob, queue, inventory, multi-order bundles) is a distinct problem | Claiming "maker = just another adapter" under-scopes maker sizing and smuggles an unspecced feature into this plan |
| Explicit RCK objective (argmax log-growth), not just a constraint | A constraint alone is satisfied by many quantities including zero; the maximand must be stated to make the size deterministic | Specifying only the drawdown constraint leaves the target undefined — an implementer would invent the objective |
