# Feature Specification: Thin NautilusTrader Boundary for Current Decision Evidence

**Issue**: [#1354](https://github.com/seungpyoson/bolt-v2/issues/1354)
**Pull Request**: [#1505](https://github.com/seungpyoson/bolt-v2/pull/1505)
**Branch**: `codex/1354-current-evidence-rebuild`
**Status**: Proposed — external architecture review is required before implementation

## Purpose

PR #1505 replaces the previous decision-evidence format with a current-only,
typed, durable contract. During implementation review, Bolt's capital-admission
runtime feed was found to maintain its own live-order set, client/venue order
mapping, terminal-event history, and order-lifecycle snapshot.

That is too much authority. NautilusTrader (NT) already owns order lifecycle,
fills, positions, cache state, execution reconciliation, and venue translation.
Bolt must remain a thin fail-closed admission and evidence layer over those NT
surfaces.

This specification closes that boundary once. It does not add another event
journal, order state machine, reconciliation engine, or compatibility path.

## Selected Architecture

Bolt computes a typed capital-admission projection from:

1. a canonical NT account/order/position snapshot;
2. Bolt's committed admission evidence and reservation policy; and
3. provider-specific readiness facts that NT does not expose, such as venue
   allowance or an accepted raw-venue attestation.

NT events may trigger recomputation. Bolt does not replay those events into a
second order-lifecycle model.

An accepted venue snapshot is an attestation that NT reconciliation observed
the complete admission-relevant venue universe. It does not become a second
live order authority. A venue/NT disagreement closes admission; Bolt does not
merge, timestamp-rank, or choose between competing order histories.

The governing safety rule is:

> No disagreement, missing relation, event ordering, crash, or restart may
> construct new-risk authority. When the authoritative inputs cannot be joined,
> admission is unreconciled and new risk is unavailable.

## Authority Contract

| Concern | Sole authority | Bolt's permitted use | Bolt must not |
|---|---|---|---|
| Order existence and status | NT cache/order model | Read a canonical snapshot | Maintain a parallel live-order set |
| Fill and remaining quantity | NT order/fill state | Derive current reservation liability | Reconstruct fill progress from a private event ledger |
| Position and account state | NT cache/portfolio | Derive exposure and available capital | Maintain a competing position or balance history |
| Adapter and venue reconciliation | NT | Wait for completion and inspect the result | Implement an alternate reconciliation engine |
| Venue raw snapshot | Provider readiness boundary | Attest NT snapshot completeness; supply provider-only spendability facts | Replace NT lifecycle state or become a live order ledger |
| Bolt action authorization | Current decision evidence | Prove that Bolt authorized a risk-changing action | Infer authorization from NT state alone |
| Capital reservation policy | Bolt submit admission | Compute liability for evidence-attributed NT orders | Redefine NT order lifecycle |
| Admission readiness | Bolt submit admission | Permit new risk only after the join succeeds | Remain ready during any unresolved mismatch |

## Required State

Bolt may retain only state it owns:

- committed decision-evidence projections;
- Bolt reservation identifiers and policy inputs;
- the latest successfully constructed admission projection;
- provider-only readiness facts not represented by NT;
- a typed reconciled/unreconciled admission result and health reason.

NT snapshots and projection are evaluated on the NT owner thread. A provider
worker may publish an immutable readiness fact and may monotonically revoke
admission while reprojection is pending. It may not read NT's thread-confined
cache, mutate lifecycle, or reopen admission. Only a fresh projection on the NT
owner thread may construct reconciled admission capability.

The following current structures are not valid long-lived Bolt authorities and
must be removed or reduced to ephemeral values derived from one NT snapshot:

- a Bolt-maintained live-order universe;
- a Bolt-maintained client-order-to-venue-order lifecycle map;
- terminal-order history used to decide which NT orders exist;
- an order-lifecycle snapshot selected by source strings or timestamps;
- event-derived position deltas that compete with the NT position snapshot.

Deduplication used solely to make a Bolt evidence append idempotent may remain,
but it must not decide NT lifecycle or admission state.

## User Scenarios and Acceptance

### Story 1 — Safe startup reconstruction (P1)

After NT startup reconciliation completes, Bolt joins the canonical NT snapshot
to committed admission evidence and provider readiness facts.

**Acceptance scenarios**

1. Given every admission-relevant NT open order has matching committed Bolt
   attribution and the accepted venue attestation names the same venue-order
   universe, the projection is reconciled and may construct admission
   capability.
2. Given an NT open order has no committed Bolt attribution, startup remains
   unreconciled and constructs no new-risk capability.
3. Given the venue attestation and NT snapshot differ in either direction,
   startup remains unreconciled and constructs no new-risk capability.
4. Given no capital-admission runtime is configured, unrelated execution
   remains unchanged.

### Story 2 — Thin live lifecycle handling (P1)

NT remains responsible for applying order and fill events. Bolt recomputes its
admission projection from the resulting canonical NT state.

**Acceptance scenarios**

1. A live or terminal NT event may trigger recomputation but cannot directly
   add or remove an order from a Bolt-owned lifecycle set.
2. A fill changes Bolt liability only through the current NT order/fill state
   joined to the matching Bolt reservation.
3. An unknown or unjoinable order/fill closes admission and emits typed health;
   it does not create a guessed reservation or silently release liability.
4. Duplicate or reordered NT events produce the same Bolt projection as the
   canonical NT snapshot and do not require Bolt event-order logic.

### Story 3 — Provider attestation without parallel authority (P1)

Venue truth verifies the completeness of NT's reconciled view and provides only
provider facts absent from NT.

**Acceptance scenarios**

1. A matching venue/NT order identity set permits the evidence join to proceed.
2. A mismatching set closes admission without mutating NT-derived lifecycle,
   positions, or reservations.
3. A later matching attestation permits a fresh full projection; it does not
   patch the previous mismatch incrementally.
4. Capture failure or semantic divergence leaves risk-reducing behavior under
   its existing policy but cannot authorize new risk.
5. A snapshot arriving on the provider worker revokes admission until the NT
   owner thread consumes it and completes a fresh projection.

### Story 4 — Restart equivalence (P1)

Restart reconstruction produces the same admission decision as an uninterrupted
run given the same canonical NT state, committed evidence, policy, and provider
readiness facts.

**Acceptance scenarios**

1. No process-local event history is required to reconstruct admission.
2. A crash before an NT lifecycle transition is reflected in canonical state
   produces the pre-transition projection.
3. A crash after NT reflects the transition produces the post-transition
   projection.
4. Machine-evidence corruption continues to fail activation; observation
   corruption remains non-authoritative.

## Transition Contract

Each row describes the only permitted Bolt reaction. “Reproject” means build a
complete result from current authorities; it never means mutate a shadow order
state.

| Input or boundary | Required Bolt behavior | Forbidden behavior |
|---|---|---|
| NT reconciliation not complete | Admission unreconciled | Infer completeness from an empty cache |
| Accepted venue snapshot before NT reconciliation | Retain only as readiness input; do not open admission | Treat it as live lifecycle state |
| Provider snapshot arrives on its worker thread | Publish immutable input and revoke admission pending NT-thread projection | Read NT cache or reopen admission from the worker |
| NT reconciliation complete; venue/NT sets match | Join NT orders to Bolt evidence and reproject | Copy the set into a second live-order ledger |
| Venue-only order | Admission unreconciled; typed mismatch | Ignore it or invent a Bolt order |
| NT-only order | Admission unreconciled; typed mismatch | Treat the venue snapshot as stale and continue |
| NT order without Bolt attribution | Admission unreconciled | Infer authorization from order presence |
| Bolt attribution without an NT open order | Inert for current reservation reconstruction | Invent an NT order or live reservation |
| NT live-order event | Trigger projection from canonical NT state | Append to a Bolt lifecycle set |
| NT fill event | Record required Bolt evidence, then derive liability from canonical NT state | Maintain an independent filled-quantity history as authority |
| NT terminal event | Derive absence/status and reservation outcome from canonical NT state | Remove from a Bolt order set solely because a raw callback arrived |
| Duplicate/reordered callback | Same projection as canonical NT state | Add timestamp/source precedence logic |
| Venue capture failure/divergence | Close new-risk admission; publish typed health | Modify NT lifecycle or choose the latest source |
| Restart | Rebuild from NT + committed evidence + provider readiness | Require process-local event or terminal history |
| Any unclassifiable input | Fail closed with a typed reason | Default, wildcard repair, or best-effort admission |

## Functional Requirements

- **FR-001 — NT ownership**: NT MUST remain the only live authority for order
  existence, lifecycle, fills, positions, accounts, portfolio state, adapter
  behavior, and reconciliation.
- **FR-002 — No shadow OMS**: Bolt MUST NOT maintain a parallel order-lifecycle
  state machine, live-order universe, terminal-order ledger, or event-derived
  position authority.
- **FR-003 — One projection**: Capital admission MUST consume one typed
  projection built from a single canonical NT snapshot, committed Bolt
  evidence, Bolt reservation policy, and provider-only readiness facts.
- **FR-004 — Events are triggers**: NT callbacks MAY trigger projection or
  evidence recording but MUST NOT become a second lifecycle source.
- **FR-005 — Attestation only**: Venue open-order data MUST be used only to
  attest NT reconciliation completeness. It MUST NOT replace or incrementally
  mutate NT lifecycle state.
- **FR-006 — Exact join**: Every admission-relevant NT open order MUST join to
  its committed Bolt attribution. Any missing, duplicate, or contradictory
  relation MUST leave admission unreconciled.
- **FR-007 — No disagreement authority**: Venue/NT mismatch, unknown external
  activity, unavailable canonical state, or failed provider capture MUST expose
  zero new-risk capability.
- **FR-008 — Reprojection**: Recovery from a mismatch MUST use a fresh complete
  projection. No patch, fallback, cached-success inheritance, or latest-source
  selection is permitted.
- **FR-009 — Restart equivalence**: Process-local callback history MUST NOT be
  necessary for restart reconstruction.
- **FR-010 — Same path**: Live and backtesting MUST use the same admission
  projection types and decision rules. Test-only data construction may supply
  NT-equivalent snapshots but not an alternate reducer.
- **FR-011 — Existing evidence guarantees**: Current-only identity closure,
  machine/observation separation, append/sync receipt honesty, poisoning,
  positive finite caps, process ownership, component handles, and hard-cutover
  behavior MUST remain unchanged unless this specification explicitly requires
  a compatible signature change.
- **FR-012 — Delete replaced paths**: Existing source/timestamp arbitration and
  shadow lifecycle mutation paths MUST be deleted, not retained behind a mode,
  feature, fallback, or compatibility adapter.
- **FR-013 — Typed failure**: Every rejected join or unavailable authority MUST
  produce a stable typed reason suitable for tests and operator health.
- **FR-014 — No adjacent implementation**: Rotation, total retained capacity,
  retirement, durable ordinals, and restart append-retry exact-once remain
  owned by #1385 and MUST NOT be implemented here.
- **FR-015 — Thread ownership**: Canonical NT snapshot reads, projection, and
  construction of reconciled admission capability MUST occur on the NT owner
  thread. Provider workers MAY revoke admission and publish immutable readiness
  inputs, but MUST NOT read NT's thread-confined cache or reopen admission.

## Explicit Non-Goals

- Reimplementing NT reconciliation or an order management system.
- Adding a Bolt acknowledgement journal.
- Persisting venue snapshots as lifecycle authority.
- Defining new provider protocols or adapter behavior.
- Changing execution, submission, cancellation, or strategy intent mechanics.
- Adding a compatibility decoder for pre-cutover evidence.
- Implementing #1385 capacity or exact-once work.
- Authorizing live cutover.

## Evidence Requirements

Tests must verify Bolt behavior, not source text.

- A table-driven authority-join suite covers every transition-contract row.
- Permutation tests cover duplicate, delayed, reordered, and missing callbacks
  while holding the canonical NT snapshot constant; the result must not change.
- Differential tests compare uninterrupted and restart projections.
- Integration tests prove a venue/NT/evidence mismatch exposes zero new-risk
  capability and a later complete match uses a fresh projection.
- Concurrency tests prove a provider update revokes admission before handoff,
  cannot reopen it on the worker, and reopens only after NT-thread projection.
- Existing evidence, settlement, poison, shutdown, cap, BTE, and generated
  contract tests remain green.
- Exact-head advisory formatting, production clippy, root/BTE tests, and
  release builds are terminal green before native review.

No source-scanning or implementation-shape tests may be introduced.

## Success Criteria

- **SC-001**: Every transition-contract row has executable behavioral evidence.
- **SC-002**: Duplicate or reordered callbacks cannot change admission when the
  canonical NT snapshot is unchanged.
- **SC-003**: Every disagreement or missing join exposes zero new-risk
  capability.
- **SC-004**: Restart and uninterrupted projections are equal for the same
  authoritative inputs.
- **SC-005**: No production component requires a Bolt-maintained open-order,
  terminal-event, or position-delta history to decide admission.
- **SC-006**: Live and BTE use one projection and one decision path.
- **SC-007**: External architecture review has no substantiated unresolved
  Critical, High, or Medium finding.
- **SC-008**: Final internal review identifies no remaining duplicate NT
  authority, fallback, or unmodeled lifecycle boundary.
- **SC-009**: No provider or background thread can read NT's thread-confined
  cache or construct reconciled admission capability.

## Relations and Scope

This is the NT-boundary closure required for PR #1505's current-only
decision-evidence slice. PR #1505 still does not claim to close all of #1354.

Issue #1385 retains rotation, total retained capacity, retirement, durable
ordinals, and restart append-retry exact-once.

The external live-cutover prerequisites and accepted loss of pre-cutover
recovery continuity remain unchanged.
