# Feature Specification: Positional Sizing Engine (family/venue/instrument-agnostic; submits to the risk-reservation substrate)

**Feature Branch**: `712-positional-sizing-engine`
**Created**: 2026-06-25 (replaces the round-2 draft after the review-driven split)
**Status**: Draft — under review (consolidated three-module review: #973 substrate, #712 sizer, #724 calibration)
**Tracking**: #712. Depends on #711 (rename) and the risk-reservation substrate (`specs/973-risk-reservation-substrate/973-risk-reservation-substrate-spec.md`); armed live by #688.
**Input**: A real positional sizer that decides *how much* to trade from a calibrated edge and a bankroll, then turns that target into an admitted order through the shared substrate. The atomic risk-reservation machinery is NOT here — it is the substrate. This spec is the sizing math plus the one coordinator that turns a target into an action.

## Overview

Today there is no real sizer: `bolt_v3_sizing.rs::choose_robust_size` scales a fixed dollar notional by an EV ratio (not bankroll-proportional), and the admission component only validates an already-chosen quantity. This spec adds a sizing engine that:

1. Computes a **target terminal exposure** from a selectable sizing model on **one seam** (config-selected, no separate code path): **risk-constrained Kelly** — the target growth model that scales to the edge, armed only once calibration (#724) provides a trustworthy edge — and **fixed fraction of equity** — the calibration-free safe mode that caps capital per trade and needs no edge-accuracy input. Each policy epoch selects ONE primary model: the launch epoch selects fixed-fraction directly (calibration isn't built yet). Fixed-fraction is risk-limited *without* calibration evidence — never proof the edge is trustworthy — and Kelly is the goal. There is no automatic or silent fallback: if a Kelly epoch's attestation is missing or invalid the default is no-trade, and switching to fixed-fraction requires an explicit, approved fallback in that epoch (FR-015). ONE path. NO DUAL PATHS.
2. Sizes the **complete target position** (existing exposure + the change), then emits the **delta** — never per-order-delta sizing.
3. Projects the target to an exact candidate using the substrate's shared evaluator and advisory view, **mints unforgeable provenance**, submits to the substrate's atomic gate, and runs a bounded retry + reduction protocol via the **one coordinator** that owns target→action.
4. Is a **shared, family/venue/instrument-agnostic module**: one sizer serves many strategies across multiple instances, venues, and instruments. Binary/taker is simply the **first** registered adapter, not a special case — its payoff structure (and any family's) lives only in a sealed, registered adapter that derives terminal cash flows from the active descriptor. Nothing venue-, instrument-, or strategy-specific lives in the sizer core.

The sizer **consumes** an edge and a coverage band; it NEVER measures calibration accuracy (owned by #724/#723). Each module does ONE job.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Fixed-fraction-of-equity: the calibration-free safe mode (Priority: P1, launch + fallback)

The engine sizes a target as an appetite fraction of reference growth wealth, then lets the substrate enforce every hard headroom. Fixed dollar notional is gone (fixed dollars = hidden rising leverage after a drawdown). The model is off by default and selected by one config dial.

**Why this priority**: It is risk-limited without any edge-accuracy input, so the launch policy epoch selects it as the primary model (calibration #724 isn't built). It is a calibration-free safe mode, not evidence the edge is trustworthy, and it is never an automatic fallback — switching to it from a Kelly epoch requires an explicit, approved fallback in that epoch (FR-015). Kelly is the target growth model, gated on calibration.

**Independent Test**: With a known equity and appetite fraction, assert the target equals the appetite allowance projected to feasibility, and that after a simulated drawdown the dollar size falls (no rising leverage).

**Acceptance Scenarios**:

1. **Given** a configured appetite fraction ρ and reference growth wealth W, **When** the model runs, **Then** its unconstrained allowance is exactly ρ·W and the final size is that target projected to substrate-enforced feasibility — the model consumes no headrooms itself.
2. **Given** a drawdown that lowers equity, **When** the model runs again, **Then** the dollar size is lower (fraction-of-equity, not fixed notional).

### User Story 2 - Stateful target sizing (size the position, emit the delta) (Priority: P1)

The engine sizes the **complete target terminal position** — including existing filled exposure and reachable pending exposure in the same instrument — and emits only the delta to reach it. It never sizes a new order in isolation.

**Why this priority**: Sizing the incremental order while ignoring existing same-instrument exposure resurrects per-decision sizing: each slice "looks safe" while the aggregate breaches the constraint. This is the exact failure the engine exists to prevent.

**Independent Test**: With existing exposure already near the target, submit a new signal; assert the emitted delta brings the *aggregate* to target (often small or zero), not a fresh full-size order.

**Acceptance Scenarios**:

1. **Given** existing exposure that already consumes most of the target, **When** a new signal arrives, **Then** the engine emits only the delta to the aggregate target, not a full-size order.
2. **Given** conflicting same-instrument pending orders, **When** sizing, **Then** the engine either evaluates the target against the conservative reachable pending-exposure envelope or requires those pendings cancelled/reconciled first; the order delta is target minus reconciled effective exposure.

### User Story 3 - The coordinator is the only target→action authority (Priority: P1)

One `SizingAdmissionCoordinator` is the sole component that turns a target into a live action. It projects the target to an exact candidate, mints provenance, submits to the substrate, and on a stale-view/contention rejection it discards, refreshes the view, re-verifies, recomputes, and retries within a bounded budget before terminating in a structured no-trade.

**Why this priority**: If more than one component can turn intent into an order, there are two sizing authorities and the single-path guarantee is gone. Bounded retry with a hard no-trade ceiling keeps the chokepoint safe without spinning.

**Independent Test**: Force repeated stale-view rejections; assert the coordinator refreshes and retries up to the bounded ceiling, then emits a structured no-trade — and that no other component can construct a risk-increasing candidate.

**Acceptance Scenarios**:

1. **Given** a view that goes stale between preview and commit, **When** the coordinator submits, **Then** it refreshes and recomputes, retrying up to the configured ceiling, then returns a structured no-trade.
2. **Given** strategy/adapter code, **When** it attempts to construct a risk-increasing admission candidate directly, **Then** it cannot (construction is private to the coordinator / behind an opaque permit) — a compile-time failure.

### User Story 4 - Family-agnostic core with a registered, sealed adapter (Priority: P1)

The core consumes a generic `TargetPosition` and opaque, adapter-supplied canonical scenario economics. Binary/taker semantics — modeled-state partition, probabilities, payoff — live ONLY in a registered, sealed family adapter, and the adapter derives terminal cash flows from the active descriptor (not its own copy).

**Why this priority**: The user's hard constraint is no hardcoding to any strategy, venue, or symbol. Encoding binary structure in the core violates it and blocks reuse by the maker and future families.

**Independent Test**: Grep the core for any family/venue/symbol branch (none); swap in a second registered adapter and assert the coordinator, models, and substrate path are unchanged.

**Acceptance Scenarios**:

1. **Given** the binary/taker adapter, **When** the core sizes and admits, **Then** no core component branches on a family/venue/symbol name; binary structure is confined to the registered adapter.
2. **Given** a modeled state s, **When** the adapter computes its net P&L Πₛ, **Then** Πₛ is derived from the exact active descriptor version bound to the sizing decision — there is no second cash-flow source.

### User Story 5 - Risk-constrained Kelly on the worst-case edge: the target growth model (Priority: P1, gated on calibration #724)

The target growth model sizes by risk-constrained Kelly on the conservative (worst-case) band end. It evaluates the moment constraint on **post-trade target wealth**, requires strictly positive wealth ratios, and arms only behind a config-loaded band-coverage attestation (produced offline by the calibration scoreboard #724) plus a named model-risk cap. When calibration is unavailable or the attestation fails, the default is no-trade; the only alternative is a configured, approved fallback to the fixed-fraction safe mode — Kelly never runs on an unverified edge.

**Why this priority**: RCK on the worst-case edge is best-in-class for the single bet (sizing at the worst-case band end is already the box-DRO solution at fraction 1 — a separate ¼–½ multiplier double-counts). But band MISSPECIFICATION is a distinct risk that must be guarded separately, not by re-adding a folklore fraction.

**Independent Test**: With no band-coverage attestation, assert the Kelly model yields no-trade. With an attestation, assert sizing satisfies Σ πₛ·Rₛ(x_target)^(−κ) ≤ 1 on the aggregate target and that a precondition violation (cost ≥ wealth) is rejected rather than producing a NaN.

**Acceptance Scenarios**:

1. **Given** the Kelly model selected but no valid band-coverage attestation, **When** sizing runs, **Then** the result is no-trade (the named lower-ceiling fallback applies only if explicitly configured and approved).
2. **Given** κ = ln β / ln α and modeled states S_model, **When** sizing runs, **Then** Rₛ(x_target) = 1 + Πₛ(x_target)/W is finite and strictly positive for every s (precondition C(q) < W enforced), and the constraint is evaluated on the complete target wealth, not the order delta.
3. **Given** S_model (the sizing states) and S_stress (the full terminal set used by the substrate for hard feasibility), **When** sizing runs, **Then** the model uses S_model for its objective and never substitutes it for the substrate's S_stress feasibility check.

### User Story 6 - Extension-seam conformance (the maker is a separate feature) (Priority: P2)

This engine MUST prove its harness is reusable: a future registered sizing policy reuses the same coordinator, substrate, evaluator, ledger, and token path with only a different registered payoff adapter. This spec does NOT claim a maker needs only a payoff adapter — maker SIZING (fill probability, queue state, inventory, cancellation policy, multi-order reservation bundles) requires its own feature specification. In scope here is the conformance seam, not maker sizing.

**Why this priority**: One harness across families is the whole point of the agnostic core, and the seam must be provably reusable; but maker sizing is a distinct problem and must not be smuggled in as "just another adapter."

**Independent Test**: Register a second, trivial conformance adapter; assert the coordinator and substrate path are the same code with only the adapter swapped. Maker sizing is out of scope and tracked as a separate feature.

**Acceptance Scenarios**:

1. **Given** a second registered conformance adapter, **When** it sizes and admits, **Then** it uses the same coordinator/service/ledger/token path; only the payoff adapter is family-specific — proving the seam, not maker sizing.

### Edge Cases

- All-in fee/slippage-adjusted cost makes the edge sub-break-even, the size sub-min-lot, or the snapshot stale → zero-size (no trade).
- A risk-reducing close is computed while the positive-edge gate would block it → the close routes through the substrate SafetyAction path and is NOT blocked by the edge gate.
- Reference growth wealth would exceed conservative liquidation equity → the model's allowance is still bounded by substrate-enforced conservative-equity headrooms (the model cannot out-size feasibility).
- The appetite fraction or a model parameter is set to a catastrophic-but-valid value → rejected by the substrate's policy envelope (allowed ranges/invariants) before activation.
- Two simultaneous signals in correlated instruments → each sizes its own target, but the substrate's atomic per-bucket reservation (not the sizer) prevents the aggregate breach.

## Requirements *(mandatory)*

### Functional Requirements

**Sizing models**

- **FR-001**: The engine MUST provide selectable sizing models behind one seam — fixed-fraction-of-equity (the calibration-free safe mode; launch + fallback) and risk-constrained Kelly (the target growth model, gated on calibration #724) — chosen by config. Both are first-class on the one seam; the safe model MUST be a selectable model, not a separate code path. ONE path.
- **FR-002**: A sizing model MUST compute only a model allowance from probability/payoff, reference growth wealth, and model controls (e.g. `model_stress_allowance = ρ · reference_growth_wealth`). It MUST NOT consume equity-floor, governor, collateral, bucket, order, or position headrooms, and MUST NOT multiply any hard headroom by an appetite factor. Feasibility is the substrate's job.
- **FR-003**: The engine MUST size the COMPLETE target terminal position (existing filled exposure + reachable pending exposure + the change) and emit the delta. Per-order-delta sizing is prohibited. Before increasing risk, the engine MUST either evaluate the target against the conservative reachable pending-exposure envelope or require conflicting same-instrument pendings cancelled and reconciled first; the admission quantity is target minus reconciled effective exposure.
- **FR-004**: Launch default MUST be fixed fraction of equity (NOT fixed notional). Fixed dollar sizing is prohibited as a default because it hides rising leverage after a drawdown.

**Risk-constrained Kelly**

- **FR-010**: The RCK model MUST solve an explicit optimization, not merely satisfy a constraint. Over the registered modeled states S_model and the permitted target domain X, the target MUST be x* ∈ argmax_{x∈X} Σ_{s∈S_model} πₛ · ln(Rₛ(x)) subject to Σ_{s∈S_model} πₛ · Rₛ(x)^(−κ) ≤ 1, with κ = ln β / ln α and explicit 0 < α < 1, 0 < β < 1, where Rₛ(x) = W_s_post_target(x) / reference_growth_wealth and W_s_post_target(x) includes current marked wealth, the terminal value change of existing filled exposure, the terminal value and execution cost of the target adjustment, and any same-instrument pending exposure treated as still reachable. The zero-risk target (no risk increase) MUST be feasible; ties MUST resolve toward lower absolute stress loss. The constraint is evaluated on post-trade target wealth, never the order delta.
- **FR-011**: Every Rₛ(x) MUST be finite and strictly positive for every modeled state s at the chosen target — this is the binding positivity requirement. The aggregate precondition C(q) < W (modeled cost below wealth) MAY be used only as an additional necessary screen; with existing exposure it does NOT by itself prove state-wise positivity and MUST NOT be treated as sufficient. A state whose post-target wealth is non-positive, or any non-finite Rₛ, MUST reject the size (no per-state return of −1 raised to a non-integer power; no NaN).
- **FR-012**: S_model (the model's sizing states) MUST be DISTINCT from S_stress (the full terminal set — void/dispute/oracle-failure/haircut/default — used by the substrate for hard feasibility). The model MUST NOT substitute S_model for the substrate's feasibility check. Band end MUST be side-aware (long → lower band end; short → upper).
- **FR-013**: Sizing at the worst-case band end MUST be treated as the conservatism budget (box-DRO at fraction 1); the engine MUST NOT additionally apply a fractional-Kelly multiplier for the same risk. Band MISSPECIFICATION MUST be guarded separately: the Kelly model MUST arm only behind a valid, config-loaded `BandCoverageAttestation` produced offline by the calibration scoreboard (#724 FR-011 — config bound into the substrate's prepared epoch, not a live dependency) AND the model-risk cap of FR-014. The attestation's bound fields (model + band-method versions, family/segment, side, horizon, eligibility pass, validity interval) MUST match the sizing decision; a stale, mismatched, ineligible, expired, revoked, or tampered attestation MUST be treated as absent. On absent/invalid attestation the result MUST follow the model-selection rule FR-015 (default no-trade).
- **FR-014** (model-risk cap): `ModelRiskCap` MUST be a fully defined, separately named cap guarding band misspecification, distinct from the band-coverage treatment. Its specification MUST state its unit, its reference wealth, an absolute and a fractional limit, its application order relative to the RCK constraint, and its behavior after a drawdown. It MUST be enforced independently of the attestation and MUST be tested as a binding cap (a case where it, not the RCK constraint, sets the size).
- **FR-015** (model selection and fallback — no silent fallback): Each policy epoch MUST explicitly select one primary sizing model. The launch epoch MAY select fixed-fraction directly. When an epoch selects Kelly and its attestation becomes missing, expired, mismatched, or revoked, the default result MUST be no-trade. Switching to fixed-fraction is permitted ONLY when that exact fallback model, its own (lower) ceilings, and its operator approval are present in the active policy epoch; no automatic or silent fallback is permitted. Fixed-fraction MUST be described as risk-limited without calibration evidence — never as evidence the declared edge is trustworthy.

**Coordinator and provenance**

- **FR-020**: One `SizingAdmissionCoordinator` MUST be the ONLY component that turns a target into an action. It MUST preview candidate quantities through the substrate's provenance-free `RiskPreviewInput` evaluated by the shared kernel (substrate FR-025) over the advisory view, select the exact quantity, then mint provenance and construct the admission candidate (provenance is minted AFTER selection, never before), submit to the substrate, and on stale-view/contention rejection discard → refresh view → re-verify → recompute → retry within a bounded budget → terminate in a structured no-trade. Strategy/adapter code MUST construct only sizing-intent values.
- **FR-021**: The coordinator MUST mint the unforgeable `SizingDecisionPermit` the substrate requires, binding decision id, sizing-policy id+version, signal/model/attestation bindings, source view version, target position, exact quantity, instrument/side/execution bounds, config/policy epoch, and expiry. In Rust, the risk-increasing candidate constructor MUST be private to the coordinator (module visibility / sealed trait) so strategy crates fail at compile time.
- **FR-022**: A risk-reducing close MUST route through the substrate's SafetyAction path and MUST NOT be blocked by the positive-edge gate. The positive-edge gate applies only to risk-increasing admission.

**Family-agnostic placement**

- **FR-030** (agnostic): The core (models, coordinator) MUST consume a generic `TargetPosition` and opaque canonical scenario economics; it MUST NOT branch on a family, strategy, venue, or symbol name. Binary/taker semantics (S_model partition, probabilities, payoff) MUST live ONLY in a registered, sealed family adapter. Verified by review/grep.
- **FR-031** (single cash-flow authority): For every modeled state s, the adapter MUST derive Πₛ from the substrate's `ActiveDescriptorView` (substrate FR-024) for the exact active descriptor version bound to the sizing decision (the substrate's descriptor is the sole terminal-cash-flow authority). A registered policy MAY select/partition descriptor state ids into S_model, attach forecast probabilities/ambiguity sets, apply utility/constraints, and combine descriptor cash flows with the version-bound execution-cost curve — but MUST NOT independently encode or override cash flows.
- **FR-032**: Model/adapter selection MUST be data-driven from the descriptor/registration (no family/strategy conditional in the coordinator).

**Execution hygiene and scope**

- **FR-040**: The engine MUST use a fee/slippage-adjusted all-in cost; break-even MUST be at that cost; it MUST zero-size on sub-edge, sub-min-lot, or stale snapshot. Orders MUST be limit-price / max-cost bounded — no market orders. The cost MUST be expressed behind a curve-shaped interface (cost as a function of size) so an impact-aware (depth-walking) cost can replace the scalar later without changing the models or the seam. The launch scalar per-unit cost MAY be used ONLY when it is a conservative upper bound on the all-in unit cash outlay for every fillable unit up to the candidate quantity under the admitted limit price and max-total-cost bounds (worst admitted price plus every fee — not top-of-book or expected slippage); otherwise the implementation MUST evaluate the executable quantity-dependent cost curve. The book/fee-model versions used by the cost function MUST bind to the sizing decision and the admission candidate, and quantity rounding or final candidate construction MUST trigger a complete post-cost edge recheck (zero-size if it falls sub-break-even).
- **FR-041** (one job): The engine MUST NOT measure calibration accuracy; it consumes a calibrated edge and coverage band and never scores market accuracy. Calibration is #724/#723.
- **FR-042** (no hardcodes): Every model parameter, appetite fraction, ceiling, and threshold MUST be runtime config (TOML), fail-closed when missing or outside the substrate's policy envelope.
- **FR-043** (off by default; extension seam): The engine MUST default to off; live arming is gated by #688. A future registered sizing policy MUST be able to reuse the same substrate/coordinator/evaluator/token path with only a different registered payoff adapter (extension-seam conformance). This spec does NOT claim a maker needs only a payoff adapter; maker sizing (fill probability, queue, inventory, multi-order bundles) is a separate feature specification.

### Key Entities *(include if feature involves data)*

- **TargetPosition**: the generic, family-agnostic target terminal exposure the core sizes to; carries no family/venue/symbol identity.
- **SizingIntent**: what strategy/adapter code may construct; never a risk-increasing candidate.
- **SizingModel**: the selectable seam — fixed-fraction-of-equity (calibration-free safe mode; launch + fallback) and risk-constrained Kelly (target growth model, gated on calibration #724).
- **RegisteredPayoffAdapter**: the sealed, registered family adapter; owns S_model/probabilities, derives Πₛ from the active descriptor.
- **SizingAdmissionCoordinator**: the one target→action authority; mints provenance, runs bounded retry + reduction.
- **BandCoverageAttestation**: the versioned, machine-verifiable artifact defined by #724 FR-011, produced offline and bound into the substrate's prepared epoch; gates Kelly arming. Absence/invalidity ⇒ the FR-015 model-selection result (default no-trade). Not a live dependency.
- **ModelRiskCap**: the separately named, fully specified cap of FR-014 (unit, reference wealth, absolute + fractional limits, application order, post-drawdown behavior) guarding band misspecification, enforced independently of the attestation.
- **SizingDecisionPermit**: the unforgeable provenance the coordinator mints for the substrate (defined by the substrate spec; minted here).

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A sizing model's output is its allowance projected to substrate feasibility; the model formula contains no headroom term — verified by review and a test asserting the model consumes no headrooms.
- **SC-002**: With existing same-instrument exposure, the emitted delta brings the aggregate to target (not a fresh full-size order) — proven by a stateful-sizing test.
- **SC-003**: RCK evaluates the constraint on post-trade target wealth (existing exposure included), every Rₛ is finite and strictly positive, and a cost ≥ wealth precondition violation is rejected (no NaN) — proven by tests.
- **SC-004**: With no band-coverage attestation, the Kelly model yields no-trade — proven by a test.
- **SC-005**: Πₛ derives from the active descriptor; there is no independent payoff-vector source in the engine — verified by review/grep.
- **SC-006**: No family/venue/symbol branch in the core; binary structure is confined to the registered adapter; the maker reuses the same coordinator/substrate path — verified by review/grep.
- **SC-007**: A risk-reducing close routes through the SafetyAction path and is not blocked by the edge gate — proven by a test.
- **SC-008**: After a simulated drawdown, the fixed-fraction model's dollar size falls (no rising leverage) — proven by a test.
- **SC-009**: ONE path — the safe model is a selectable model, not a separate code path; one coordinator owns target→action — verified by review.
- **SC-010** (RCK optimization): The RCK target maximizes expected log growth subject to the drawdown constraint, the zero-risk target is feasible, and `ModelRiskCap` binds where it is the tighter limit — proven by tests (including a case where the cap, not the constraint, sets the size).
- **SC-011** (no silent fallback): With a Kelly epoch whose attestation is missing/expired/mismatched, the result is no-trade unless the epoch explicitly carries an approved fixed-fraction fallback with its own lower ceilings — proven by tests of both branches.
- **SC-012** (conservative launch cost): The launch scalar cost is a proven conservative upper bound on all-in unit outlay up to the candidate quantity under the admitted limit price, and quantity rounding triggers a post-cost edge recheck — proven by a test that fails if the scalar understates worst-case fill cost.

## Assumptions

- The risk-reservation substrate (`specs/973-risk-reservation-substrate/973-risk-reservation-substrate-spec.md`) exists and owns the atomic gate, the descriptor authority, the advisory view, provenance verification, and the SafetyAction path. This engine depends on it and does not re-implement any of it.
- #711 has renamed the legacy admission component to the `capital_admission` gate, freeing the `position_sizer` name for this real sizer.
- Band-coverage attestations are produced **offline** by #724/#723 and promoted to config; this engine consumes them as config (not a live call) and never measures calibration. Until #724 is built, Kelly cannot arm and the engine runs the fixed-fraction safe mode.
- The binary/taker adapter is implemented first. The harness is proven reusable via extension-seam conformance (User Story 6); maker sizing is a separate feature specification, not part of this engine.
- `bolt_v3_sizing.rs::choose_robust_size` (fixed-notional × EV) is retired by this engine; there is no dual sizing path.

## References

- `specs/973-risk-reservation-substrate/973-risk-reservation-substrate-spec.md` — the substrate this engine submits to (job 2 of the split).
- Risk-constrained Kelly: Busseti, Ryu, Boyd, "Risk-Constrained Kelly Gambling" (arXiv:1603.06183).
- Code anchors (at `main`): `bolt_v3_sizing.rs::choose_robust_size` (retired), `bolt_v3_position_sizer.rs::evaluate_position_sizing` (renamed by #711), `bolt_v3_sizing_state.rs::NtDerivedSizingState`, signal sources `bolt_v3_taker_updown_signal.rs`, `bolt_v3_binary_outcome_edge.rs`, `bolt_v3_taker_pricing.rs`.
- Dependency chain: #711 → #973 substrate → #712; #688 arms live. Kelly (the target model) is gated on calibration #724/#723 (offline → config).
