# Feature Specification: Risk-Reservation Substrate (gate-owned safety ledger, family/venue/instrument-agnostic)

**Feature Branch**: `712-positional-sizing-engine` (design authoring; substrate implementation branches from its own tracking issue)
**Created**: 2026-06-25
**Status**: Draft — under round-6 external review
**Tracking**: #973. Depends on #711; consumed by #712; armed live by #688.
**Input**: The shared, atomic risk-reservation foundation that every order-producing family submits to. Split out of #712 after five rounds of external adversarial review found #712 meshed two jobs: the sizing math (#712) and this substrate.

## Overview

The substrate is the **gate-owned safety ledger**: the single bounded context that, before any order is admitted, **atomically reserves** collateral, the realized-loss budget, equity-floor stress loss, and every applicable concentration bucket **on one coherent risk-state version**. It is the only place these can be reserved atomically, so it is the only place the portfolio loss limit can be a real control rather than a reporting number.

It is a **shared foundation**. Taker (P1), maker (P2), and any future family submit to the *same* substrate through the *same* contracts; only the sizing *model* is family-specific. NO DUAL PATHS, ONE serialization domain, ONE source of truth for instrument terminal economics.

The substrate **computes all authoritative risk itself**. Callers supply primitive order facts only; any risk number a caller includes is diagnostic and is never the basis for a reservation. This closes the failure mode that drove the split: a gate that faithfully reserves caller-understated risk is not a safety control.

**Lineage — this is an evolution of the consolidated gate, not a new system.** The "position sizer" (#507) was never just sizing: it bundled the loss governor (#505), the capital-reservation ledger (#504), and the admission gate, then consolidated (#658) with loss-halt and (#673/#738) with the kill switch's cancel/flatten/halt — deliberately, to end the fragmentation that had three reimplemented loss governors. This substrate is the NEXT iteration of that one consolidated gate (which #711 renames to `capital_admission_gate`). It REUSES the existing loss governor, kill switch, reservation ledger, NT runtime feeds, and restart recovery as single sources (FR-064), and adds only what five review rounds proved missing: atomic reservation across all dimensions on one version, descriptor-owned terminal economics, type-bound provenance, and prepared-epoch cutover. It MUST NOT re-fragment that stack.

This spec covers the substrate (job 2). The sizing math (job 1) is `specs/712-positional-sizing-engine/712-positional-sizing-engine-spec.md`.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Atomic compare-and-reserve on one version (Priority: P1, the core gate)

A family's coordinator submits a primitive admission candidate (instrument, side, exact quantity, order type/TIF, max unit price, max cash outlay, version stamps, provenance). The substrate, inside one serialized critical section, recomputes every authoritative risk quantity from its own state and the active descriptor, checks all hard limits, and either reserves atomically and returns an admission token or returns a structured rejection. No partial reservation is ever observable.

**Why this priority**: This is the substrate. Without an atomic compare-and-reserve on one coherent version, simultaneous correlated bets sized against one bankroll breach the loss budget before any reactive governor fires — the unanimous #1 risk across all review rounds.

**Independent Test**: Drive concurrent candidates whose individual risks are each within budget but whose sum exceeds it; assert the budget is never breached and exactly the admissible prefix is reserved, on one monotonic version.

**Acceptance Scenarios**:

1. **Given** free budget for one of two simultaneous candidates, **When** both are submitted concurrently, **Then** exactly one reserves and the other is rejected with a structured reason; the budget is never observed breached.
2. **Given** a candidate carrying caller-computed risk figures that understate the true risk, **When** it is submitted, **Then** the substrate reserves the risk it recomputes itself and the caller figures change nothing (and a diagnostic mismatch is recorded).

### User Story 2 - Descriptor authority: the substrate owns terminal economics (Priority: P1)

The substrate resolves the active instrument-risk descriptor version itself, computes loss against the descriptor's certified terminal-state enumeration, and fails closed when a descriptor is uncertified, when a candidate references a non-active version, or when a realized outcome maps to no certified state.

**Why this priority**: All authoritative risk now flows from one input. If that input is stale, incomplete, or caller-selected, the substrate reserves a correct number for the wrong economics — "authoritative but wrong." This is the dominant residual safety surface.

**Independent Test**: Register two descriptor versions; submit a candidate naming the stale one; assert rejection. Present a descriptor missing a terminal state; assert the unknown-state envelope applies and risk-increasing admission for that instrument halts.

**Acceptance Scenarios**:

1. **Given** an active descriptor version V2 and a candidate naming V1, **When** submitted, **Then** the substrate resolves V2 independently and rejects the mismatch; a caller cannot select the authoritative version.
2. **Given** a descriptor with no non-expired coverage attestation, **When** any candidate for that instrument is submitted, **Then** it fails closed (no admission).
3. **Given** a realized outcome that maps to no certified terminal state, **When** it is observed, **Then** the conservative unknown/other envelope (no better than the maximum venue-enforceable loss) applies AND further risk-increasing admission for that instrument is disabled until reconciliation and descriptor review complete.

### User Story 3 - Crash, restart, and venue-event reconciliation (Priority: P1)

After a crash mid-commit, or on reconnect, the substrate recovers risk state and the reservation ledger to one coherent version, reconciles against the venue's accepted-order truth, and never double-reserves or double-submits.

**Why this priority**: A reservation ledger that loses coherence on restart silently understates exposure. Local idempotency keys do not bind to one venue order; without venue-side idempotency a retransmit creates a second live order against a single reservation.

**Independent Test**: Kill the process between reservation and venue acknowledgement; restart; assert exactly one live order, one reservation, and a coherent version with no orphaned capacity.

**Acceptance Scenarios**:

1. **Given** a durable submission-intent written before the first external send, **When** the process restarts before acknowledgement, **Then** reconciliation produces exactly one venue order (idempotent create or proof-of-absence before retransmit) and one matching reservation.
2. **Given** venue fills and cancellations that arrived during downtime, **When** the substrate reconciles, **Then** risk state and the ledger reflect them on one monotonic version before any new risk-increasing admission is processed.

### User Story 4 - SafetyAction: de-risking is always available (Priority: P1)

A de-risking action (cancel an order, reduce-only close a position) is a separate, closed operation type. The substrate authorizes it by recomputing — from its own authoritative order-level state — that the action does not increase risk for any still-possible fill outcome, even when new-risk admission is frozen.

**Why this priority**: If de-risking shares the new-risk admission path, a frozen or overloaded gate blocks the system from reducing risk — the exact moment it must not. And a permissive "anything non-increasing" rule is broad enough to smuggle arbitrary trades.

**Independent Test**: Freeze risk-increasing admission; submit a reduce-only close; assert it is admitted. Submit a disguised risk-increasing or substitution action through the SafetyAction path; assert rejection.

**Acceptance Scenarios**:

1. **Given** risk-increasing admission frozen, **When** a `ReduceOnlyCloseExistingPosition` is submitted, **Then** it is admitted after the substrate recomputes the pointwise-nonincrease and operation-identity proofs from authoritative state.
2. **Given** a SafetyAction that would open a new instrument, increase absolute position, cross sign, create a resting quote, or substitute exposure, **When** submitted, **Then** it is rejected (the operation set is closed).

### User Story 5 - Prepared-epoch atomic cutover (Priority: P2)

A descriptor or policy change is staged as one immutable prepared bundle, fully validated and revalued against current open risk, then activated atomically at one linearization point. No mixed old/new descriptor, classifier, fee, model, or limit set is ever observable.

**Why this priority**: Descriptor resolution is defined against the *active* epoch, yet a new epoch cannot safely activate until its descriptors are certified and affected open risk is revalued. Without a staged cutover this is circular and admits half-installed states.

**Independent Test**: Stage an epoch that raises required reserves; assert that during cutover no candidate commits under mixed old/new data, and that if revaluation breaches a headroom, activation completes with risk-increasing admission disabled.

**Acceptance Scenarios**:

1. **Given** a prepared epoch that increases required reserves on open positions, **When** it activates, **Then** affected open orders are cancelled/reduced and risk-increasing admission resumes only after the ledger is consistent under the new epoch.
2. **Given** a partial revaluation failure, **When** cutover is attempted, **Then** the system remains in a no-new-risk state and alerts; it never partially commits the new epoch.

### User Story 6 - Versioned advisory read view (Priority: P2)

The substrate publishes an immutable, versioned advisory view (headrooms, exposure envelope, outstanding reservations, caps, all version stamps, expiry) that the sizing engine reads to preview a target. The view grants no capacity; possession reserves nothing.

**Why this priority**: The sizing engine needs substrate-owned state to project a target, but reads must not become a second source of truth or a stale precheck that the commit silently trusts.

**Independent Test**: Read a view, mutate state, submit against the stale view; assert the commit recomputes and rejects the stale candidate with no side effect.

**Acceptance Scenarios**:

1. **Given** a published view at version N, **When** state advances to N+1 and a candidate bound to N is submitted, **Then** the commit recomputes under N+1 and rejects the stale candidate cleanly.

### User Story 7 - Bounded work and lifecycle priority under load (Priority: P2)

The authoritative critical section does bounded, I/O-free work over pre-resolved immutable data. Venue-event ingestion, reconciliation, and SafetyActions take priority over risk-increasing admission. Under overload the substrate sheds risk-increasing work fail-closed and never delays lifecycle truth.

**Why this priority**: One serialization domain is correct for safety but is a throughput chokepoint. If the commit path can do unbounded work or admission can starve lifecycle updates, the implementer's natural fix is a parallel path — which reintroduces the precheck-vs-commit race the substrate exists to kill.

**Independent Test**: At the configured maximum event/strategy/bucket/descriptor load, assert p99/max latency bounds for venue-state updates, SafetyActions, and admission; drive past the supported envelope and assert risk-increasing work is shed without reservation corruption or lifecycle starvation.

**Acceptance Scenarios**:

1. **Given** sustained load within the supported offered-load envelope, **When** measured, **Then** lifecycle and SafetyAction latency bounds hold and every conforming admission producer receives bounded service under the documented fair-queue policy.
2. **Given** load above the envelope, **When** measured, **Then** risk-increasing admission sheds fail-closed and triggers an operational alert; lifecycle truth is never delayed.

### User Story 8 - Reservation lifecycle through fills, cancels, and settlement (Priority: P1)

A reservation is NOT released when an order is merely sent, partially fills, or is locally cancelled; it transitions through an explicit state machine driven only by authoritative venue/settlement truth. A partial fill moves filled quantity from open-order reservation to filled-position exposure and RETAINS the conservative reservation for the unfilled remainder; only venue-confirmed cancellation/expiry or completed reconciliation releases the remainder; filled-position stress and loss reservations persist until the terminal outcome is final.

**Why this priority**: The largest residual production risk is premature or incorrect release of a multi-dimensional reservation after a partial fill, cancel request, replace, expiry, or settlement transition — which would let a second order reuse collateral or loss capacity while the first can still fill or still carries terminal risk, breaching the very budget the atomic admission exists to enforce.

**Independent Test**: Drive permutations — partial fill → cancel → late fill; replace in flight; settlement revision; duplicate and out-of-order events; sequence gaps — and assert aggregate reserved risk never falls merely because an order partially filled, no capacity is released without authoritative confirmation, and any gap/out-of-order event moves the affected state to reconciliation and blocks new risk.

**Acceptance Scenarios**:

1. **Given** an open order that partially fills, **When** the fill is processed, **Then** filled quantity becomes filled-position exposure at actual fill economics, the unfilled remainder keeps its conservative reservation, and `risk_state_version` advances exactly once.
2. **Given** a local cancel request or local timeout, **When** it occurs, **Then** no capacity is released until the venue confirms non-fillability or reconciliation completes; a late fill after the cancel request is still covered.
3. **Given** a duplicate or out-of-order venue event, **When** ingested, **Then** duplicates are idempotent and an out-of-order event or sequence gap moves the affected state to `ReconciliationRequired` and blocks new risk-increasing admission.

### Edge Cases

- A candidate arrives whose required bucket set the caller mis-declared → the substrate-derived classification is authoritative; the caller declaration is diagnostic only.
- Two worst-case loss metrics diverge (equity-floor stress loss from current conservative liquidation equity vs governor realized loss from entry cost basis) → both are reserved; neither scalar substitutes for the other.
- A descriptor is revoked while orders rest and positions are open → immutable descriptors plus mandatory revaluation block risk increases until affected state is revalued.
- A syntactically valid but catastrophic config (e.g. an appetite multiplier of one, or a pool stress limit equal to all equity) → rejected by the policy envelope's allowed ranges and cross-field invariants before activation.
- The reduction feasible-fill set is combinatorial → the proof domain is bounded; authorization uses a proven monotone envelope, a conservative relaxation, bounded enumeration, or fails closed pending reconciliation.
- The descriptor producer and the descriptor approver are the same automated identity → rejected; coverage approval requires separation of duties.

## Requirements *(mandatory)*

### Functional Requirements

**Atomic reservation core**

- **FR-001**: The substrate MUST expose ONE pool-scoped, linearizable compare-and-reserve transaction that, on a single coherent `risk_state_version`, atomically reserves collateral, realized-loss-budget capacity, equity-floor stress-loss capacity, every applicable concentration bucket, and position/order capacity — or reserves nothing. No partially reserved state may be observable.
- **FR-002**: `AdmissionCandidate` MUST carry only primitive order facts (intent/idempotency key, pool id, instrument id + expected descriptor version, side, exact quantity, order type/TIF, max unit price, max cash outlay, venue/fee model versions, source view version, config/policy epoch, signal/model/attestation bindings, sizing provenance, expiry). It MUST NOT carry authoritative risk quantities. Any caller-supplied risk figure MUST be treated as diagnostic only and MUST NOT affect any reservation; a mismatch against the recomputed value MUST be recorded and MUST fail closed where it would otherwise relax a limit.
- **FR-003**: A single substrate-owned, pure `RiskEvaluator` MUST compute every authoritative risk quantity (collateral, incremental equity-floor stress loss, incremental governor realized loss, post-fill position/order usage, global and per-bucket usage, hard-limit results). The SAME function MUST serve the advisory preview and the authoritative commit; it MUST perform no mutable reads and no I/O.
- **FR-004**: A single substrate-owned `RiskClassifier` MUST derive the COMPLETE required concentration-bucket set authoritatively from the active descriptor's canonical attributes and active policy. Caller-declared bucket membership MUST be diagnostic only. A missing classification attribute MUST fail closed.
- **FR-005**: The substrate MUST treat worst-case loss as TWO distinct authoritative metrics — equity-floor stress loss (from current conservative liquidation equity) and governor realized loss (from entry cost basis) — and reserve each against its own limit. One scalar MUST NOT serve both.

**Serialization and state**

- **FR-006**: Account-derived risk state and the reservation ledger MUST live in ONE serialization domain (single-writer actor or one transactional store) with ONE monotonic `risk_state_version`. Linearizability MUST be defined relative to the processed venue-event history. Two version numbers MUST NOT be treated as if they were one transaction.
- **FR-007**: Only the single state-owner component may mutate authoritative risk state.
- **FR-008**: The authoritative assess-and-reserve critical section MUST perform no external I/O, acquire no nested mutable lock, use only pre-resolved immutable descriptor/policy/fee/classifier data, perform no unbounded allocation, enforce configured maximum scenario/bucket/candidate sizes, and have a documented worst-case computational complexity.

**Descriptor authority**

- **FR-010**: The substrate MUST own an instrument-risk registry that is the SOLE authority mapping (instrument, venue, active policy epoch) to exactly one active descriptor version. The substrate MUST resolve the active version itself; a candidate MAY carry an expected version, and any mismatch MUST reject the candidate. A caller MUST NOT select the authoritative version.
- **FR-011**: The active descriptor MUST be the SOLE authority for instrument terminal cash flows, settlement fees, recoveries, collateral rules, and terminal-state identifiers. No other component may independently encode or override these. (Sizing models attach probabilities/utility and select state partitions but derive cash flows from the active descriptor — see `specs/712-positional-sizing-engine/712-positional-sizing-engine-spec.md`.)
- **FR-012**: No descriptor may become active without a non-expired coverage attestation identifying instrument family and venue, source resolution-rule hash, descriptor schema and generator versions, a complete terminal-state enumeration, payout/recovery/collateral/settlement-fee validation, required classification-attribute coverage, conservative test vectors, and issuer/approver identity, validity interval, and revocation status. Missing, expired, revoked, or mismatched certification MUST fail closed.
- **FR-013**: Descriptor production and descriptor coverage approval MUST be distinct, independently authenticated roles. The attestation MUST be machine-verifiable by the substrate and MUST name both producer and approver identities. (The substrate cannot prove real-world terminal-state completeness; completeness is a registration-time gate owned by a named certification authority, not a runtime check.)
- **FR-014**: Every descriptor MUST include an explicit unknown/other terminal-state envelope whose payout and recovery assumptions are no better than the maximum loss the venue's enforceable collateral and contract rules permit. An outcome mapping to no certified state MUST use this envelope AND disable further risk-increasing admission for the affected instrument until reconciliation and descriptor review complete.
- **FR-015**: Descriptors MUST be immutable. Activating, correcting, or revoking a descriptor version MUST trigger revaluation of all affected open orders, reservations, and positions before risk-increasing trading resumes; if revaluation breaches any headroom, the substrate MUST block risk increases and permit only SafetyActions.

**Read seam and provenance**

- **FR-020**: The substrate MUST publish an immutable, versioned, advisory `RiskSizingView` exposing `risk_state_version`, reconciliation readiness, reference growth wealth, conservative liquidation equity, free collateral, equity-floor and governor headrooms, global stress-loss headroom, per-bucket headrooms, filled positions, a pending-fill exposure ENVELOPE (filled / min-reachable / max-reachable per instrument and side, not a scalar), outstanding reservations, caps, all version stamps, and expiry. The view MUST grant no capacity; possession MUST reserve nothing.
- **FR-021**: The relationship between reference growth wealth and conservative liquidation equity MUST be stated and MUST be safe by construction: reference growth wealth MUST NOT exceed conservative liquidation equity for the purpose of any reservation, and all hard headrooms MUST be enforced against substrate-authoritative conservative equity independent of the model's reference wealth.
- **FR-022**: A risk-increasing `AdmissionCandidate` MUST carry unforgeable sizing provenance (a permit constructible only by the owning sizing coordinator — see #712). The substrate MUST reject every risk-increasing candidate lacking valid, unconsumed provenance. Permit validation, idempotency lookup, reservation commit, and permit consumption MUST be one atomic operation; a replay with the same idempotency key MUST return the existing result and MUST NOT authorize a second economic order.
- **FR-023**: The provenance trust model MUST be stated explicitly. Phase 1 MUST use an in-process capability: coordinator and admission service share one trusted process/compilation boundary, the permit is non-serializable and unconstructible by strategy/adapter code, and no IPC or persistence boundary carries the permit itself (durable submission-intent carries admitted facts and the idempotency key, not the permit). If provenance ever crosses IPC, persistence, or restart, the permit MUST be authenticated by a coordinator-held signing/MAC key unavailable to strategies and adapters, and the substrate MUST verify issuer, key version, signature, nonce, expiry, exact bound fields, and policy epoch.
- **FR-024** (active-descriptor read contract): The substrate MUST expose an immutable `ActiveDescriptorView` bound to a `RiskSizingView`, its descriptor version, and the policy epoch, exposing the opaque terminal-state ids and per-state cash-flow vectors a registered sizing policy needs to derive Πₛ. Possession MUST grant no authority to select or change the descriptor; it resolves the SAME active version the commit will use.
- **FR-025** (provenance-free preview): The advisory path MUST accept a primitive `RiskPreviewInput` (candidate facts WITHOUT admission provenance) and return a `RiskAssessment` while reserving nothing; the fully formed `AdmissionCandidate` (which carries provenance) MUST be constructed only AFTER the exact quantity is selected and the `SizingDecisionPermit` is minted. The SAME pure kernel (FR-003) MUST evaluate `RiskPreviewInput` and the primitive facts inside `AdmissionCandidate`. Equivalence is scoped: for identical explicit inputs evaluated on the same `risk_state_version`, preview and commit MUST return identical assessments. The preview is advisory only and MAY differ from the committed assessment whenever risk state or version has advanced between preview and commit; the commit ALWAYS re-evaluates atomically on the current version and is the sole authority. An earlier advisory result MUST NOT be treated as binding on the commit.
- **FR-026** (type-enforced submission boundary): The live execution boundary MUST accept ONLY an `AdmittedOrder`. An `AdmittedOrder` MUST be constructible only by the submission authority, through the atomic transition of the matching reservation from Reserved to Submitted (FR-043). A `RawOrder`, an `AdmissionCandidate`, a bare quantity, or any strategy/adapter-owned order type MUST NOT implement or reach the live submit trait — bypass MUST fail at compile time and at the dependency boundary. (FR-022/023 fence *construction* of a risk-increasing candidate; this FR fences the *live send* — the two ends of the chain.)

**SafetyAction**

- **FR-030**: `SafetyAction` MUST be a closed sum type containing only `CancelExistingOrder`, `ReduceOnlyCloseExistingPosition`, and venue-required administrative actions that cannot create fillable exposure. A SafetyAction MUST NOT introduce a new instrument, increase absolute position in any instrument, cross position sign, create a new resting quote, or substitute one exposure for another.
- **FR-031**: For every SafetyAction the substrate MUST recompute, from authoritative order-level state and the conservative feasible set of all still-possible fills, both (a) the pointwise-nonincrease proof over every possible pending-fill state and (b) an operation-identity proof that the quantity is bounded by existing reducible exposure after all required cancellations reconcile. The coordinator's proof is advisory and is NEVER the basis for admission. SafetyActions MUST be admissible while risk-increasing admission is frozen.
- **FR-032**: The min/max reachable-position fields in the view are advisory summaries. SafetyAction authorization MUST use authoritative order-level state, cost basis, reserved execution terms, and the conservative feasible set; endpoint-only checking is permitted ONLY where the loss and bucket functions are proven monotone over the entire feasible set. The SafetyAction verifier MUST bound the number of uncertain orders / fill dimensions it accepts and MUST use a proven monotone envelope, a conservative polynomial-time relaxation, bounded exact enumeration, or fail-closed rejection pending further cancellation/reconciliation. Unbounded fill-state enumeration inside the critical section is prohibited.

**Lifecycle, idempotency, reconciliation**

- **FR-040**: A durable submission-intent record MUST be written atomically before the first external send. Venue submission MUST be idempotent (idempotent create bound to an immutable client order id) or MUST prove absence before retransmission. A local idempotency key MUST NOT be assumed to bind to exactly one venue order.
- **FR-041**: On restart and on reconnect, the lifecycle reconciler MUST reconcile durable intents, the ledger, and risk state against the venue's accepted-order truth to one coherent version before any new risk-increasing admission is processed.
- **FR-042**: Venue-event ingestion, reconciliation, cancellation confirmations, and SafetyAction processing MUST have priority over risk-increasing admission. Queues MUST be bounded; risk-increasing work MUST be shed/rejected under overload; lifecycle truth MUST never be delayed to preserve new-order throughput.
- **FR-043** (reservation state machine): Every reservation MUST have an explicit lifecycle state machine covering at least Reserved → Submitted → Open → PartiallyFilled → Filled → Settled, plus CancelRequested, CancelConfirmed, ExpiredConfirmed, SubmissionUnknown, and ReconciliationRequired. Transitions MUST be driven only by authoritative venue/settlement truth and reconciliation, never by local intent alone.
- **FR-044** (partial fills): A fill event MUST atomically (a) transfer the filled quantity from open-order reservation to filled-position exposure using actual fill economics, (b) retain the full conservative reservation for the remaining fillable quantity, and (c) advance `risk_state_version` exactly once. Aggregate reserved risk MUST NOT fall merely because an order partially filled.
- **FR-045** (cancellation and replacement): A local cancel request, local timeout, or local expiry MUST NOT release capacity. Only authoritative venue confirmation or completed reconciliation may release the unfilled remainder. A cancel/replace MUST reserve the conservative combined old-plus-new exposure until the old order is confirmed non-fillable (a late fill after a cancel request MUST remain covered).
- **FR-046** (settlement release): Filled-position stress and governor reservations MUST remain active until the terminal outcome is final under the active descriptor and reconciliation is complete. A settlement revision MUST be processed as an authoritative transition, not an unreserve.
- **FR-047** (event integrity): Duplicate venue/settlement events MUST be idempotent. An out-of-order event or a sequence gap MUST move the affected state to `ReconciliationRequired` and block new risk-increasing admission for the affected scope until reconciled.

**Epoch and policy activation**

- **FR-050**: Policy, descriptor map, classifier version, fee model, sizing-policy versions, approvals, and attestations MUST first form an immutable `PreparedPolicyEpoch` whose internal references resolve without consulting a partially active epoch. Before cutover the substrate MUST validate the entire bundle, execute descriptor and classifier coverage checks, revalue all affected positions/orders/reservations under the prepared epoch (after draining queued venue events to establish accurate state, before any new admission), and determine the post-cutover admission state.
- **FR-051**: Activation MUST atomically replace the old policy epoch with the prepared epoch at one state-owner linearization point. No mixed old/new descriptor, classifier, fee, model, or limit set may be observed. If the new epoch makes current exposure non-compliant, activation MAY complete only with risk-increasing admission disabled and SafetyAction enabled. Partial revaluation failure MUST leave a no-new-risk state and alert; it MUST NOT partially commit.
- **FR-052**: The view, descriptor resolution, sizing provenance, candidate, reservation, and admission token MUST all bind one atomic policy epoch. A policy or descriptor revocation MUST invalidate unsubmitted work and block risk increases until affected state is revalidated.
- **FR-053**: Every live configuration MUST validate against a separately versioned, approval-bound `SafetyPolicyEnvelope` defining allowed ranges, cross-field invariants, permitted models/fallbacks, maximum activation horizon, required approvals, and environment/pool scope. A syntactically valid value outside the envelope MUST fail closed before activation.

**Cross-cutting**

- **FR-060** (agnostic): The substrate MUST NOT branch on a family, strategy, venue, or symbol name. All family/venue/instrument specifics MUST enter only through the active descriptor (canonical scenario economics, collateral rules, classification attributes) and the registered sizing policy. Verified by review/grep.
- **FR-061** (no hardcodes): Every ceiling, threshold, freshness/expiry bound, and limit MUST be runtime configuration (TOML), fail-closed when missing or out of envelope. No strategy/venue/symbol/family ceiling may be compiled into code. Any genuine compiled invariant MUST be enumerated and justified.
- **FR-062** (off by default): Enforcement MUST default to off; arming for live use is gated by #688. Fail-closed behavior MUST hold whether enforcement is on or off (off = no admission, not open admission).
- **FR-063** (one job per module): The substrate MUST be one bounded context decomposed into single-job components with one-way dependencies — instrument-risk registry, pure risk kernel, pure classifier, sole-mutation state owner, reservation ledger, admission service, lifecycle reconciler, view publisher, submission authority. Only the state owner mutates authoritative risk state. It MUST NOT be one mixed-responsibility module.
- **FR-064** (reuse the existing kill switch + loss governor — single source, no dual path): The substrate MUST reuse the existing loss governor (`bolt_v3_loss_governor.rs`, `bolt_v3_loss_halt_actions.rs`, `bolt_v3_loss_protection.rs`) as the SOLE realized-loss accumulator and the existing kill switch (`bolt_v3_kill_switch*.rs`: latch, action router, cancel, flatten, store) as the SOLE NT-native cancel/flatten/halt path. The "governor realized loss" metric (FR-005) MUST read the loss governor's number, not recompute a parallel one. SafetyAction cancel and reduce-only-close (FR-030/FR-031) MUST execute through the existing kill-switch cancel/flatten orchestration, not a second path. The substrate's distinct role is the PROACTIVE pre-trade atomic reservation the kill switch lacks (the kill switch is reactive/post-breach; the substrate reserves *before* admission); it MUST sit behind the existing kill-switch latch. The admission chain MUST be: static syntax/type validation → the substrate's atomic compare-and-reserve, into which the kill-switch latch and loss-governor state are ingested as versioned inputs (FR-066) and ALL stateful caps (notional, count, bucket, loss) are evaluated on the one coherent `risk_state_version` (FR-065). There MUST be no post-substrate cap stage that can reject after a token issues. NO second loss accumulator, NO second cancel/flatten path, NO strategy-local kill logic, NO stateful gate downstream of the reservation. (#673 already removed three duplicate loss governors; this requirement prevents reintroducing that dual path.)
- **FR-065** (gate ordering — every stateful cap inside the transaction): Every mutable or state-dependent order, position, notional, count, loss, and bucket limit MUST be evaluated INSIDE the FR-001 compare-and-reserve transaction on the one coherent `risk_state_version`. Only static syntax/type validation may precede it. No risk or capacity gate may reject AFTER an `AdmissionToken` is issued — there is no post-reservation rejection path, and therefore no reservation rollback to specify.
- **FR-066** (reused-state binding): The existing kill-switch latch and loss-governor values remain their sole authoritative sources (FR-064), but their state/version MUST be ingested into the state owner's coherent event history and bound to `risk_state_version` BEFORE admission — they MUST NOT be read as mutable values checked independently just before the commit. The commit decision and the reservation MUST be taken on one version that already reflects the latched/governor state.
- **FR-067** (Phase-1 pool ownership fence): Before processing any authoritative mutation of a pool's risk state or reservation ledger, the owning process MUST hold an exclusive lease for that `pool_id` AND a monotonically increasing fencing token issued by a configured lease authority. Every durable risk-state and reservation mutation MUST be validated against the current fencing token at the authoritative store / state owner; a write carrying a stale token MUST be rejected, so a paused or partitioned former owner CANNOT commit after ownership transfers. Lease-acquisition failure, renewal failure, ambiguous lease status, or lease loss MUST immediately prohibit new admission and submission by that process (fail closed). A successor owner MUST reconcile durable intents, orders, fills, positions, and reservations (FR-041) before enabling new risk. The lease authority is a configured dependency, not a hardcoded backend; selecting and naming it is a blocking S0 deliverable (see plan). This is the normative form of the Phase-1 single-owner-per-pool decision recorded in Assumptions; Phase 2's named transactional store (FR-006) MUST preserve the same fencing property across processes.

### Key Entities *(include if feature involves data)*

- **AdmissionCandidate**: primitive order facts + version stamps + provenance + expiry; the write contract into the gate. No authoritative risk numbers.
- **RiskSizingView**: immutable, versioned, advisory read contract; headrooms + pending-fill envelope + reservations + caps + versions. Grants no capacity.
- **RiskState**: account-derived risk state under the sole state owner; carries the monotonic `risk_state_version`.
- **ReservationLedger entry**: an atomic reservation across all dimensions, bound to one version and one admission token; extends today's `bolt_v3_capital_reservation.rs::ReservationLedger`.
- **InstrumentRiskDescriptor**: immutable, versioned, certified; the sole authority for terminal cash flows, fees, recoveries, collateral, state ids, classification attributes, and the unknown-state envelope.
- **DescriptorCoverageAttestation**: machine-verifiable certification (enumeration completeness, validation, producer/approver identities, validity, revocation).
- **SizingDecisionPermit**: unforgeable provenance binding a sizing decision to its exact candidate; minted only by the sizing coordinator (#712); consumed once.
- **AdmissionToken**: version-bound proof of a committed reservation; the only authority to submit the bound order.
- **SafetyAction**: closed sum type for de-risking; admitted by recomputed dual proof.
- **PreparedPolicyEpoch / SafetyPolicyEnvelope**: the staged, prevalidated activation bundle and its allowed-range/invariant guard.
- **RiskBucket**: a substrate-derived concentration dimension with its own headroom and cap.
- **ActiveDescriptorView**: immutable, epoch-bound read handle (FR-024) exposing terminal-state ids + per-state cash-flow vectors for sizing; grants no descriptor-selection authority.
- **RiskPreviewInput / RiskAssessment**: the provenance-free preview input and its result (FR-025); the same pure kernel evaluates it and the committed candidate.
- **AdmittedOrder**: the only type the live submit boundary accepts (FR-026); constructible only by the submission authority via a Reserved→Submitted transition.
- **Reservation lifecycle state**: the FR-043 state machine each reservation occupies (Reserved … Settled, plus Cancel/Expired/Unknown/ReconciliationRequired).
- **PoolOwnershipLease**: the fenced Phase-1 ownership grant (FR-067) — an exclusive lease per `pool_id` plus a monotonically increasing fencing token issued by a configured lease authority and validated on every durable mutation; a stale token cannot commit.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Under concurrent correlated candidates whose sum exceeds the loss budget, the budget is never observed breached and exactly the admissible prefix reserves on one monotonic version — proven by a concurrency/property test.
- **SC-002**: No caller-supplied risk number changes any reservation — proven by a test feeding understated diagnostic figures and asserting the recomputed reservation and a recorded mismatch.
- **SC-003**: A candidate naming a non-active descriptor version is rejected; an uncertified descriptor and an unmapped terminal state both fail closed — proven by tests.
- **SC-004**: A crash between reservation and venue acknowledgement recovers to exactly one live order, one reservation, and a coherent version — proven by a restart/reconciliation test.
- **SC-005**: A SafetyAction (reduce-only close / cancel) is admitted while risk-increasing admission is frozen, and a disguised risk-increasing/substitution action is rejected — proven by tests; the substrate recomputes both proofs from authoritative state.
- **SC-006**: An epoch cutover never exposes a mixed old/new state, and a partial revaluation failure leaves a no-new-risk state — proven by a cutover test.
- **SC-007**: The commit critical section has a documented worst-case complexity and does no I/O; under load above the supported envelope, risk-increasing admission sheds fail-closed while lifecycle/SafetyAction latency bounds hold — proven by a capacity/overload test.
- **SC-008**: No dual paths — one serialization domain, one risk-state version, one cash-flow authority (the descriptor), one reservation ledger, one realized-loss accumulator (the existing loss governor), one NT cancel/flatten path (the existing kill switch) — verified by review/grep (FR-064).
- **SC-009**: No family/venue/symbol name branch in the substrate; every family/venue fact resolves from the descriptor or config — verified by review/grep.
- **SC-010**: Every ceiling/threshold is runtime config and fails closed when missing or out of the policy envelope — verified by review and a fail-closed test.
- **SC-011** (reservation lifecycle): Across partial-fill → cancel → late-fill, replace-in-flight, settlement-revision, duplicate-event, and sequence-gap permutations, aggregate reserved risk never falls without authoritative confirmation and no capacity is released prematurely — proven by lifecycle permutation tests.
- **SC-012** (bypass prevention): Only an `AdmittedOrder` reaches the live submit boundary; strategy, adapter, replay, and operator modules cannot submit a live risk-increasing order without one — proven by compile-fail and dependency-boundary tests.
- **SC-013** (gate ordering): Every stateful cap is evaluated inside the compare-and-reserve and no gate rejects after token issuance — verified by review and a test asserting no post-token rejection path exists.
- **SC-014** (split-brain fencing): With two candidate owners started for one pool, ownership transferred to the second while the first is paused, and the stale first owner then resumed, exactly one owner may commit and every stale-token durable mutation or submission attempt is rejected — proven by a split-brain fencing test (FR-067).

## Assumptions

- #711 has freed the `capital_admission` gate seam (or the substrate names the seam it will own once #711 lands); this substrate extends the existing reservation primitive rather than forking it.
- The bankroll surface (`bolt_v3_sizing_state.rs::NtDerivedSizingState`: free collateral, equity, open orders, positions, loss snapshot) is the input to risk state; the substrate adds the atomic reservation and version that surface lacks today.
- Multi-instance serialization is DECIDED for launch. Phase 1 supports exactly ONE process owner per capital pool, enforced by the normative pool-ownership fence (FR-067: an exclusive lease + monotonically increasing fencing token, stale-owner writes rejected; split-brain proven by SC-014), a single-writer actor within that owner. Many strategies and many pools across many instances are supported; what Phase 1 does NOT do is let two processes concurrently mutate ONE pool's ledger. Cross-process sharing of a single pool is Phase 2: the same abstract serialization domain (FR-006) backed by a NAMED transactional store with stated isolation level, fencing, durability, and a multi-process compare-and-reserve test — a backend swap that does not change consumers. The sizing engine (#712) is stateless and instance-safe; shared-capital coherence is the substrate's job, not the sizer's.
- NautilusTrader owns execution, position/PnL, and pre-trade limits; the substrate reserves against, and reconciles with, NT's order/position truth — it does not re-implement it. NO DUAL fill or settlement truth.
- The kill switch (`bolt_v3_kill_switch*.rs`) and loss governor (`bolt_v3_loss_governor.rs`, `bolt_v3_loss_halt_actions.rs`, `bolt_v3_loss_protection.rs`) already exist on `main` (#505/#509/#673; Phases 1–2 landed, P3–P5 in flight). The substrate REUSES them as single sources (FR-064): it is the proactive pre-trade reservation layer they lack, sits behind the kill-switch latch, reads the governor's realized-loss truth, and routes SafetyActions through the kill switch's NT cancel/flatten — it does NOT duplicate or replace them. The kill switch stays the reactive emergency stop; the substrate is the proactive pre-trade gate. #634 (forced risk-reduction exit) is a SafetyAction consumer of this same path.
- The descriptor-certification authority exists as a named internal service or controlled workstream; this spec owns the substrate's verification of its attestations, not the authority's internal process.
- Calibration accuracy (edge/band correctness) is out of scope here and in #712; it is owned by #724/#723. The substrate consumes descriptor economics, never measures market accuracy.

## References

- `specs/712-positional-sizing-engine/712-positional-sizing-engine-spec.md` — the sizing math that consumes this substrate (job 1 of the split).
- Code anchors (at `main`): `bolt_v3_capital_reservation.rs::ReservationLedger`, `bolt_v3_sizing_state.rs::NtDerivedSizingState`, `bolt_v3_submit_admission.rs`, `bolt_v3_live_node.rs`, `bolt_v3_config.rs::CapitalPoolBlock`, `bolt_v3_position_sizer.rs::evaluate_position_sizing` (renamed by #711). Reused single sources (FR-064): `bolt_v3_loss_governor.rs` + `bolt_v3_loss_halt_actions.rs` (realized-loss accumulator), `bolt_v3_kill_switch*.rs` (NT cancel/flatten/halt; #509/#673).
- Dependency chain: #711 (rename → clean gate) → this substrate → #712 (sizing); #688 arms live. Reuses the kill switch (#509/#673) + loss governor as single sources (FR-064); #634's forced-reduction is a SafetyAction consumer.
