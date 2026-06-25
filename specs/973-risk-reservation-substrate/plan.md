# Implementation Plan: Risk-Reservation Substrate (gate-owned safety ledger)

**Branch**: `712-positional-sizing-engine` (design authoring; substrate implementation branches from its own tracking issue) | **Date**: 2026-06-25 | **Spec**: `specs/973-risk-reservation-substrate/spec.md`
**Input**: Feature specification from `specs/973-risk-reservation-substrate/spec.md`. Tracking: #973. Depends on #711; consumed by #712; armed live by #688.

## Summary

Build the atomic risk-reservation substrate: one bounded context that, on a single coherent risk-state version, reserves collateral + realized-loss budget + equity-floor stress loss + concentration buckets + position/order capacity before any order is admitted. It computes all authoritative risk itself from a substrate-resolved, certified instrument-risk descriptor; exposes a versioned advisory read view and a type-bound write path; owns a closed SafetyAction set; and activates descriptor/policy changes through a prepared-epoch atomic cutover. It extends the existing reservation primitive rather than forking it, and consumes NT's order/position/fill truth rather than rebuilding it.

## Technical Context

**Language/Version**: Rust (edition per workspace).
**Primary Dependencies**: NautilusTrader Rust crates at the rev **pinned in `Cargo.toml`** (single source of truth; this plan does not restate the SHA). NT owns execution, order lifecycle, venue reconciliation, portfolio/account/order/fill state, and venue wire translation — the substrate reserves against and reconciles with NT truth and MUST source-prove NT's capabilities before adding any local machinery.
**Storage**: One transactional risk-state + reservation store in a single serialization domain (single-writer actor or one transactional store), with a monotonic `risk_state_version` and durable submission-intent + reservation ledger surviving restart. Extends `bolt_v3_capital_reservation.rs::ReservationLedger`. No new external datastore unless source-proven necessary.
**Testing**: `cargo test` (unit + property + concurrency), restart/reconciliation tests, an overload/capacity test; `cargo fmt`/`clippy`/`deny` clean. Evidence class per slice below. Exact-head remote proof via `just verify-remote` before the slice is called done; no live arming here (that is #688) — live requires fail-closed evidence for invalid/missing inputs PLUS exact-head proof per AGENTS.md and Constitution IV.
**Target Platform**: Linux (EC2 LiveNode) + offline tests.
**Project Type**: Single Rust project (NT thin-layer admission substrate).
**Constraints**: NO HARDCODES (TOML, fail-closed), PURE RUST (no C/FFI/Python in the binary), NO DUAL PATHS (one serialization domain, one risk-state version, one cash-flow authority, one reservation ledger), SSM-only secrets, GROUP BY CHANGE, OFF BY DEFAULT.
**Scale/Scope**: Many concurrent thin markets and strategies submitting to one pool-scoped serialization domain; a documented supported offered-load envelope with fail-closed shedding above it.

## Constitution Check

*GATE: must pass before implementation; re-check after each slice.*

- **I. NT-First Thin Layer** — PASS with a mandatory source-proof. The substrate is a **pre-submit admission gate + reservation ledger**, which the constitution assigns to bolt. It MUST NOT rebuild NT's order lifecycle, venue reconciliation, fill/position/account truth, or a mock venue. The substrate's "lifecycle reconciler" reconciles the **bolt reservation** against NT's authoritative order/position/fill events (release/adjust reservations as NT reports outcomes); "venue-side submission idempotency" MUST be expressed through NT's order submission and client-order-id binding. **S0 first source-proves what NT provides for submission idempotency, order-state reconciliation, and restart recovery, and uses NT's surfaces; bolt adds only the pre-submit submission-intent + reservation layer NT lacks.** Any gap that forces local machinery is recorded as a source-proof finding, not assumed.
- **II. Generic Core, Concrete Edges** — PASS. The substrate is venue/family/strategy-agnostic (FR-060); all instrument/venue specifics enter only via the active descriptor and TOML-selected registry. No concrete provider/venue/symbol may leak into the kernel, classifier, state owner, or admission service (SC-009).
- **III. Single Path And Config-Controlled Runtime** — PASS. One serialization domain, one risk-state version, one cash-flow authority, one reservation ledger; off by default; every ceiling is TOML behind the policy envelope (SC-008/SC-010).
- **IV. Evidence-Driven Verification Gates** — PASS. Live stays fail-closed; the substrate is off until #688 arms it, and arming requires fail-closed evidence + exact-head proof. Each slice carries an evidence class.
- **V. Evidence Before Claims** — PASS. Every slice's done-claim maps to a named test or review/grep artifact at the exact head.
- **VI. Minimal Slice Discipline** — PASS. Slices S0–S7 are independently shippable and each fails closed.
- **VII. Research/Analytics NT-First** — N/A (not a research/dashboard surface); the substrate is the sanctioned admission gate, not a hidden submit path.

## Architecture — one bounded context, single-job components (FR-063)

One-way dependencies; only the state owner mutates authoritative risk state:

- **InstrumentRiskRegistry** — certified descriptor activation + active-version resolution (FR-010..FR-015).
- **RiskKernel** — pure evaluation; the shared evaluator for preview and commit; two loss metrics; bounded/IO-free (FR-003, FR-005, FR-008).
- **RiskClassifier** — pure, authoritative complete bucket derivation (FR-004).
- **RiskStateOwner** — sole mutation + serialization domain + monotonic version (FR-006, FR-007).
- **ReservationLedger** — reservation state transitions under the state owner; extends `bolt_v3_capital_reservation.rs` (FR-001).
- **AdmissionService** — compare → evaluate → reserve → token; atomic permit consumption (FR-001, FR-002, FR-022).
- **LifecycleReconciler** — NT-event + restart reconciliation of reservations against NT truth (FR-040, FR-041).
- **RiskViewPublisher** — immutable advisory versioned views (FR-020, FR-021).
- **SubmissionAuthority** — token consumption + admitted-order construction (FR-022, FR-040).
- **EpochManager** — prepared-epoch validation + atomic cutover + policy envelope (FR-050..FR-053); **SafetyAction** verifier with bounded reduction-proof domain (FR-030..FR-032).

## Project Structure

### Documentation (this feature)

```text
specs/973-risk-reservation-substrate/
├── spec.md   # the substrate spec
└── plan.md   # this file
```

### Source Code (new modules; extends the reservation primitive, no fork)

New substrate modules sit alongside and extend the existing `bolt_v3_` admission/reservation modules (exact paths confirmed against the live tree at implementation; convention follows `bolt_v3_capital_reservation.rs`, `bolt_v3_submit_admission.rs`, `bolt_v3_sizing_state.rs`, `bolt_v3_config.rs`, `bolt_v3_live_node.rs`). One module per single-job component above. Contracts (`AdmissionCandidate`, `RiskSizingView`, `AdmissionToken`, `SizingDecisionPermit`, `SafetyAction`, `PreparedPolicyEpoch`, `SafetyPolicyEnvelope`) live in a substrate contracts module owned here and referenced by #712 — single source of truth.

**Structure Decision**: Extend the existing reservation primitive into the single serialization domain + monotonic version; add the registry/kernel/classifier/reconciler/view-publisher/submission-authority/epoch-manager as new single-job modules. Nothing in NT is rebuilt; the legacy validate-only admission path is superseded by the atomic compare-and-reserve (no dual path).

## Slices (dependency-ordered; each fails closed; evidence class per slice)

- **S0 — Foundation, seams, NT source-proof.** Define the contracts; stand up the single serialization domain + monotonic version over the existing ledger; skeleton the single-job components; **source-prove NT submission-idempotency / order-state reconciliation / restart recovery** and record gaps. Off by default. *Evidence: review/grep (agnostic seam, single domain) + NT source-proof notes + `cargo test` skeleton; fmt/clippy/deny.*
- **S1 — RiskKernel + RiskClassifier (pure).** The shared evaluator (two loss metrics, bounded/IO-free, documented complexity) + authoritative bucket derivation. *Evidence: unit + property tests (loss-metric distinctness, bucket completeness, missing-attribute fail-closed); complexity note.*
- **S2 — Compare-and-reserve.** The atomic transaction across all dimensions on one version; the view publisher. *Evidence: SC-001 concurrency/property test (correlated candidates never breach budget) + SC-002 (caller risk numbers ignored) + stale-view rejection test.*
- **S3 — Descriptor registry + certification.** Active-version resolution, attestation verification, unknown-state envelope, immutability + revaluation, separation of duties. *Evidence: SC-003 fail-closed tests (stale version rejected, uncertified fail-closed, unmapped outcome → envelope + halt).*
- **S4 — Provenance + idempotency + reconciliation.** Atomic permit consumption; durable submission-intent before first send (via NT submission per S0); restart/reconnect reconciliation of reservations against NT truth. *Evidence: SC-004 restart test (exactly one live order + one reservation + coherent version); replay returns existing result.*
- **S5 — SafetyAction path.** Closed operation set; recomputed dual proof; bounded reduction-proof domain; admissible while frozen. *Evidence: SC-005 tests (reduce-only admitted while frozen; disguised risk-increase rejected; bounded proof domain).*
- **S6 — Prepared-epoch + policy envelope + atomic cutover.** Staged bundle validation + revaluation + atomic cutover; allowed-range/cross-field-invariant guard. *Evidence: SC-006 cutover test (no mixed old/new observable; partial-failure → no-new-risk) + catastrophic-but-valid config rejected.*
- **S7 — Bounded work + lifecycle priority + load envelope.** Lifecycle/SafetyAction priority; bounded queues; overload shedding; supported offered-load envelope + fairness. *Evidence: SC-007 capacity/overload test (latency bounds hold inside envelope; shed fail-closed above it; no lifecycle starvation).*

Live arming (enforce on, exact-head proof) is out of scope here and tracked by #688.

## Complexity Tracking

| Decision | Why needed | Simpler alternative rejected because |
|----------|------------|--------------------------------------|
| Single serialization domain | Atomic multi-dimension reserve on one version is the only way the portfolio loss cap is a real control | Advisory cross-batch checks are TOCTOU; a cap that is not atomically reserved is a reporting number, not a control |
| Substrate-owned descriptor authority | All authoritative risk flows from one input; a caller-selected or stale descriptor reserves correct numbers for wrong economics | Trusting caller-supplied risk or a caller-named version is the failure mode that drove the split |
| In-process capability provenance (P1) | Enforces a single sizing authority at compile time without IPC auth complexity | A serialized permit needs signing/MAC; deferred to the forward path until provenance must cross a process boundary |
