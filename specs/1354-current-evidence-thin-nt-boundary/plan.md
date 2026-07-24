# Implementation Plan: Thin NautilusTrader Boundary for Current Decision Evidence

**Issue**: [#1354](https://github.com/seungpyoson/bolt-v2/issues/1354)
**Pull Request**: [#1505](https://github.com/seungpyoson/bolt-v2/pull/1505)
**Specification**: [spec.md](spec.md)
**Status**: Architecture review gate — production implementation must not begin
until the external-review gate below passes

## Summary

Replace Bolt's event-mutated capital-admission lifecycle mirror with one
projection over canonical NT state, committed Bolt authorization evidence,
Bolt reservation policy, and provider-only readiness facts.

The implementation deletes duplicate lifecycle authority. It does not add a
new state-machine framework.

## Governance Check

- **NO HARDCODES**: No new runtime policy literals; existing TOML remains the
  policy authority.
- **NO DUAL PATHS**: One projection and one admission decision path for live and
  BTE. The existing shadow lifecycle mutation path is deleted.
- **NO DEBTS**: No TODO, compatibility mode, fallback, waiver, or deferred
  correctness finding.
- **NT boundary**: NT remains authoritative for orders, fills, positions,
  accounts, adapters, and reconciliation.
- **Issue scope**: Only the #1354 current-evidence slice is changed. #1385 work
  remains excluded.
- **Verification**: Behavioral and integration evidence only; no source-scanning
  tests.

## Current Implementation Gap

The current head already reconstructs startup reservations from NT cache plus
committed evidence, but the live feed also maintains:

- `live_order_attribution`;
- `client_order_ids_by_venue_order_id`;
- `accepted_venue_open_order_ids`;
- `terminal_order_ids_seen`;
- an event/source-selected `order_lifecycle`; and
- event-derived position deltas.

That state is updated directly from callbacks and then used by submit
admission. It duplicates part of NT lifecycle and creates ordering questions
that the evidence contract cannot solve.

## Target Design

### Canonical input

Introduce one typed, immutable input assembled from an NT snapshot:

- configured account identity;
- current NT open orders and their NT lifecycle fields;
- current NT positions;
- current NT account/portfolio balances;
- the NT reconciliation-complete boundary;
- committed Bolt admission attribution and reservation policy; and
- provider-only readiness facts, including venue-order-set attestation and
  spendability facts not represented by NT.

Names are implementation choices, but the type must represent a snapshot, not
an event history.

### Projection result

The pure projection returns:

- attributed current reservations and derived liabilities;
- the capital pool inputs used by admission;
- `Reconciled` or a typed `UnreconciledReason`;
- provider readiness/health inputs; and
- no callable submission authority unless all required joins succeed.

The projection does not write evidence, submit orders, mutate NT, or apply
incremental callback logic.

### Live triggers

Startup reconciliation completion, relevant NT order/fill/account/position
changes, and accepted venue truth may trigger projection.

The trigger layer obtains current NT state and invokes the same projection. A
callback payload may be recorded as Bolt evidence when the evidence contract
requires it, but it is not stored as a private lifecycle history.

The pinned NT execution engine applies an order event to its cache before it
publishes that order event to subscribers. The implementation may rely on that
specific post-apply boundary only after an integration test pins it. If any
required callback is published before the corresponding NT state is canonical,
the implementation must use an existing NT post-apply surface or stop for
architecture review; it must not compensate with a Bolt event journal.

### Provider attestation

The provider snapshot is compared to the corresponding NT snapshot as one
readiness check. Mismatch returns an unreconciled result. Provider data does
not patch the NT snapshot or survive as a second order authority.

Venue truth currently runs on a separate OS thread, while the NT cache is
thread-confined. That worker may store/send one immutable provider-readiness
input and immediately revoke admission pending reprojection. It must never read
the NT cache or reopen admission. An existing NT-thread dispatch surface must
consume the input and perform the only projection that can reopen admission.
If no suitable existing dispatch surface exists, stop for architecture review
rather than adding a second event journal.

## Dependency-Ordered Work

### Gate 0 — Freeze and externally review the architecture

1. Commit `spec.md`, this plan, `external-review-prompt.md`, and
   `external-review-resolution.md` on a clean exact head.
2. Send the identical review request to Claude, GPT, and Kimi.
3. Adjudicate every substantive finding in the resolution log.
4. Amend the specification and plan where a finding is accepted.
5. Perform one focused external re-review of the amended artifacts.
6. Implementation may begin only when no substantiated Critical, High, or
   Medium architecture finding remains.

External review is evidence, not merge authority.

### Phase 1 — Write behavioral proof against the selected boundary

Add RED tests for:

- startup with matching venue, NT, and evidence universes;
- venue-only, NT-only, and unattributed NT orders;
- duplicate and contradictory identity relations;
- duplicate, missing, delayed, and reordered callback permutations;
- NT terminal and fill outcomes with unchanged versus changed canonical state;
- capture failure/divergence;
- provider-thread revocation followed by NT-thread reprojection;
- mismatch followed by a fresh complete match;
- uninterrupted versus restart projection equality; and
- live/BTE projection equivalence.

The tests exercise public behavior and typed results. They must not inspect
source tokens, fields, or function names.

### Phase 2 — Introduce the canonical projection

1. Define the immutable NT-backed admission input and typed projection result.
2. Reuse existing NT cache/order/position/account surfaces; do not add a Bolt
   protocol adapter or journal.
3. Move reservation/evidence joins into one pure projection.
4. Represent all failure classes with closed typed reasons.
5. Preserve existing capital policy and fail-closed semantics.

Expected primary files:

- `src/bolt_v3_capital_admission.rs`
- `src/bolt_v3_capital_admission_state.rs`
- `src/bolt_v3_submit_admission.rs`
- `src/bolt_v3_live_node.rs`

### Phase 3 — Rewire startup and live triggers

1. Keep the post-NT-reconciliation startup guard.
2. Assemble one canonical NT snapshot at that boundary.
3. Compare provider attestation to that snapshot without copying it into a
   live Bolt order ledger.
4. Pin the NT cache-update-before-publication boundary with an integration
   test, then make relevant live callbacks trigger fresh projection from
   canonical NT state.
5. Route provider readiness through an existing typed NT-thread dispatch
   surface: worker-side receipt revokes, NT-thread projection alone may reopen.
6. Keep evidence append ordering and existing risk-reducing policies intact.
7. Ensure an unreconciled result removes new-risk capability before any caller
   can submit.

Expected primary file:

- `src/bolt_v3_capital_admission_runtime_feed.rs`

### Phase 4 — Delete duplicate authority

Delete the replaced production paths rather than retaining them:

- Bolt live-order lifecycle map;
- client/venue lifecycle map used as authority;
- terminal-order lifecycle history;
- source/timestamp lifecycle selection;
- event-derived position authority; and
- incremental venue/NT order-universe merge logic.

Retain only evidence-specific deduplication that does not decide NT state.

### Phase 5 — Recovery and BTE equivalence

1. Make restart use the same projection over reconstructed authoritative inputs.
2. Make BTE use the same input/result types and reducer.
3. Preserve current-only evidence codecs, consumer projections, caps, sink
   ownership, settlement replay, and poison behavior.
4. Prove no process-local callback history is needed after restart.

### Phase 6 — Documentation and verification

1. Update active runtime contracts and runbook language to state the thin NT
   boundary and fail-closed mismatch behavior.
2. Remove or rewrite claims that Bolt owns live order-lifecycle reconciliation.
3. Run targeted behavioral tests while implementing.
4. Run repository formatting and diff checks.
5. Push the exact head and rely on advisory CI for full root/BTE clippy, tests,
   and release builds.
6. Conduct one final internal adversarial review against the specification.
7. Request native code-owner review only after all findings are resolved and
   the exact head is clean and pushed.

## Requirement-to-Evidence Matrix

| Requirement | Required evidence |
|---|---|
| FR-001/FR-002 NT ownership/no shadow OMS | Behavioral callback-permutation tests and structural type review |
| FR-003 one projection | Unit tests over immutable snapshots plus live/BTE integration tests |
| FR-004 events are triggers | Same canonical snapshot yields same result for every callback ordering |
| FR-005 venue attestation only | Venue/NT mismatch tests proving no lifecycle mutation |
| FR-006 exact join | Raw-only, NT-only, duplicate, and unattributed-order tests |
| FR-007 no disagreement authority | Submission-capability absence and typed health assertions |
| FR-008 fresh recovery projection | Mismatch-then-match integration test |
| FR-009 restart equivalence | Differential uninterrupted/restart suite |
| FR-010 same live/BTE path | Shared-type compile evidence and behavior parity |
| FR-011 existing guarantees | Existing exact-head regression suites |
| FR-012 delete replaced paths | Structural design review plus absence of a second callable path; no source-scanning test |
| FR-013 typed failure | Exhaustive typed-reason tests |
| FR-014 scope | Three-dot diff and issue-scope review |
| FR-015 thread ownership | Worker-revocation/NT-thread-reopen concurrency test |

## Stop Conditions

Stop implementation and return to architecture review if:

- the design requires Bolt to interpret general order lifecycle independently
  of NT;
- a second reducer or compatibility path appears necessary;
- provider snapshots must be persisted as lifecycle authority;
- correctness requires a Bolt acknowledgement journal;
- the selected NT surface cannot provide a canonical state at the proposed
  trigger boundary;
- reopening admission would require a provider worker to read NT state or
  mutate lifecycle;
- a substantiated external Critical/High/Medium finding remains unresolved; or
- the change begins implementing #1385.

## Completion Gate

The slice is ready for native review only when:

- every requirement in `spec.md` has named evidence;
- every transition row is exercised;
- all accepted external findings are implemented;
- final internal adversarial review has no unresolved substantive finding;
- the worktree is clean and pushed;
- exact-head advisory evidence is terminal green; and
- no compatibility lane, fallback, TODO, or duplicate NT authority remains.
