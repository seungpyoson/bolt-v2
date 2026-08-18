# PR #1544 Lifecycle-First Scope-Back Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Do not delegate this plan.

**Goal:** Replace ambiguous post-submit rollback and strategy-owned recovery with lifecycle-retained shared admission and one private current-process edge-taker exposure reducer.

**Architecture:** Shared execution prepares capital and the optional maker lifecycle participant, crosses one infallible sink boundary, and treats an error returned after the NT call as `SinkInvokedUnknown`. Capital admission retains one order-ID-keyed record and one numerical ledger across incomplete projections, while the edge taker keeps only current-process claims inside a private reducer and enters `BlindRecovery` for restart or unprovable lineage. Existing NT cache, lifecycle, cancellation, and reconciliation surfaces remain authoritative.

**Tech Stack:** Rust 2024, NautilusTrader Rust API at `e4167fd1ed5ce9db06b43a81417ab4096b8b84b6`, Cargo, TOML, serde/JSONL current evidence

**Spec:** `docs/superpowers/specs/2026-08-18-pr-1544-lifecycle-first-scope-back-design.md`

## Global Constraints

- Never consult Bolt v1 and do not change the NautilusTrader pin.
- Shared execution/admission stays strategy-, venue-, and market-family-neutral; edge exposure types stay private to `binary_oracle_edge_taker`.
- Strategies continue to emit intent and consume typed outcomes; they do not gain an alternate submit, cancellation, cache, or reconciliation path.
- The reservation ledger remains the only mutable numerical liability authority; the order-ID index stores immutable attribution, exact ledger identity, phase, and phase revision.
- No timeout, cache absence, actor restart, error-string match, or `Result<()>` proves that a sink-invoked command did not exist.
- No new source-scanning test, compatibility decoder, evidence identity, generic exposure runtime, durable claim journal, cancellation algorithm, or retry loop is added.
- The behavior cutover is one production commit. Only this plan and an optional behavior-preserving exposure encapsulation may precede it.
- New behavior is developed red-green. Existing behavior-preserving movement uses the existing suite plus structural-equivalence inspection as allowed by `AGENTS.md`.
- The implementation range must be net negative for primary `if`/`match` lines and `companion_union_lines` under `src/strategies/`, including the combined edge-taker `mod.rs` plus `exposure.rs` result. The complete PR must finish below net `+250` primary lines from `e62584045629208e81d2dce1fce608720ea01fbf`.
- #869 and economics Slices 2–5 remain separate. Nothing here authorizes deploy, readiness, live operation, trading, or merge.

---

### Task 1: Establish the exact baseline and commit the plan

**Files:**
- Create: `docs/superpowers/plans/2026-08-19-pr-1544-lifecycle-first-scope-back.md`

**Interfaces:**
- Consumes: approved design head `cbb56e984d3b7b55e923f3f6aaae0b0fea1a412d`.
- Produces: a clean, reviewable execution checklist and baseline evidence before production edits.

- [x] **Step 1: Verify isolation and ancestry**

Run:

```bash
git rev-parse HEAD HEAD^
git status --short --branch
git rev-parse --git-dir
git rev-parse --git-common-dir
```

Expected: HEAD is `cbb56e984d3b7b55e923f3f6aaae0b0fea1a412d`, parent is `dc15cddc204732a4bfe8bdabb219b120b1fc8e7b`, the branch is `codex/1445-economics-cutover`, and the linked worktree is clean except for this plan.

- [x] **Step 2: Run the smallest useful baseline suite**

Run serially:

```bash
cargo test --locked --lib bolt_v3_capital_reservation::tests -- --nocapture
cargo test --locked --lib route_attempt_participant_spans_the_final_pre_sink_and_sink_transaction -- --nocapture
cargo test --locked --features test-current-evidence-inspection --lib binary_oracle_edge_taker::tests -- --test-threads=1
cargo test --locked --features test-current-evidence-inspection --test admission_orders -- --test-threads=1
cargo test --locked --features test-current-evidence-inspection --test maker_taker -- --test-threads=1
```

Expected: all commands pass at the documentation-only head. If a baseline command fails, stop and diagnose before changing production code.

Baseline evidence at `cbb56e984d3b7b55e923f3f6aaae0b0fea1a412d`: capital reservation 26/26, route-attempt participant 1/1, edge-taker 429/429, admission/orders 145/145, and maker/taker 73/73. The shared `/Volumes/CargoBuild` volume was full, so the latter integration runs used the isolated target `/private/tmp/bolt-v2-target-1544-scopeback`; no repository behavior failed.

- [ ] **Step 3: Commit the implementation plan**

```bash
git add docs/superpowers/plans/2026-08-19-pr-1544-lifecycle-first-scope-back.md
git commit -m "docs(economics): plan lifecycle-first cutover"
```

---

### Task 2: Make the capital ledger lifecycle-retentive and candidate-first

**Files:**
- Modify: `src/bolt_v3_capital_reservation.rs`
- Modify: `src/bolt_v3_capital_admission.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `src/bolt_v3_capital_admission_runtime_feed.rs`
- Test: `src/bolt_v3_capital_reservation.rs`
- Test: `tests/bolt_v3_capital_admission_runtime_feed.rs`
- Test: `tests/bolt_v3_submit_admission.rs`

**Interfaces:**
- Produces: `BoltV3SubmitReservationPhase::{Reserved, SinkInvoked, ObservedOpen}` and one typed reservation record keyed by client order ID.
- Produces: `capital_admission_state_revision: u64`, replacing `capital_admission_nt_projection_epoch` rather than adding a second counter.
- Produces: generic `ReservationLedger::remove_existing(pool_id, collateral_group_id, reservation_id)` which knows no lifecycle or strategy types.
- Produces: a candidate rebuild that accepts fresh NT evidence or an exact retained existing reservation, and swaps only after evidence succeeds and the expected revision still matches.
- Consumes later: `BoltV3SubmitAdmissionPermit::prepare_sink_invocation` in Task 3.

- [ ] **Step 1: Write failing generic-ledger tests**

Add tests proving these observable contracts:

```rust
#[test]
fn unreconciled_ledger_allows_exact_existing_rollback() {
    let pool = pool();
    let request = reservation_request("request-unreconciled-rollback");
    let mut ledger = ReservationLedger::reconciled();
    assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);
    ledger.invalidate_reconciliation();

    assert_eq!(
        ledger.rollback_uncommitted(&pool.pool_id, &request.request_id),
        Some(Decimal::new(40, 0)),
    );
    assert_eq!(ledger.live_reserved_liability(&pool.pool_id), Decimal::ZERO);
}

#[test]
fn remove_existing_ignores_pool_freshness_but_requires_exact_generic_identity() {
    let pool = pool();
    let request = reservation_request("request-terminal");
    let mut ledger = ReservationLedger::reconciled();
    assert!(ledger.reserve(&pool, &request, 1_020, None).accepted);
    ledger.invalidate_reconciliation();

    assert_eq!(
        ledger.remove_existing(&pool.pool_id, "wrong-group", &request.request_id),
        ReservationExistingRemoval::IdentityMismatch,
    );
    assert_eq!(ledger.live_reserved_liability(&pool.pool_id), Decimal::new(40, 0));
    assert_eq!(
        ledger.remove_existing(
            &pool.pool_id,
            &request.collateral_group_id,
            &request.request_id,
        ),
        ReservationExistingRemoval::Removed(Decimal::new(40, 0)),
    );
}

#[test]
fn rejected_candidate_rebuild_preserves_the_live_ledger() {
    let pool = pool();
    let live = reservation_request("request-live");
    let malformed = ReservationRequest {
        evidence_label: String::new(),
        ..reservation_request("request-malformed")
    };
    let mut ledger = ReservationLedger::reconciled();
    assert!(ledger.reserve(&pool, &live, 1_020, None).accepted);

    let candidate = ledger.build_candidate(
        &pool,
        &[ReservationRebuildEvidence::Fresh(&malformed)],
        1_020,
        None,
    );

    assert!(!candidate.decision().accepted);
    assert_eq!(ledger.live_reserved_liability(&pool.pool_id), Decimal::new(40, 0));
}

#[test]
fn candidate_rebuild_carries_an_exact_existing_reservation_without_restamping() {
    let pool = pool();
    let live = reservation_request("request-carried");
    let mut ledger = ReservationLedger::reconciled();
    assert!(ledger.reserve(&pool, &live, 1_020, None).accepted);

    let candidate = ledger.build_candidate(
        &pool,
        &[ReservationRebuildEvidence::Retained(ExistingReservationIdentity {
            pool_id: &pool.pool_id,
            collateral_group_id: &live.collateral_group_id,
            reservation_id: &live.request_id,
        })],
        9_999,
        None,
    );

    assert!(candidate.decision().accepted);
    assert_eq!(candidate.live_reserved_liability(&pool.pool_id), Decimal::new(40, 0));
    assert_eq!(candidate.observed_at_ns(&pool.pool_id, &live.request_id), Some(1_010));
}
```

The production mutations these tests catch are: reinstating the reconciliation guard on removal, applying freshness to terminal removal, clearing before validation, or reconstructing carried liability from a new request.

- [ ] **Step 2: Verify the ledger tests fail for the intended reasons**

```bash
cargo test --locked --lib bolt_v3_capital_reservation::tests -- --nocapture
```

Expected: failures name the unreconciled guard, missing `remove_existing`, destructive rebuild, or absent retained-candidate API—not fixture or parsing errors.

- [ ] **Step 3: Implement the minimal generic ledger contract**

Use substrate-neutral shapes equivalent to:

```rust
pub struct ExistingReservationIdentity<'a> {
    pub pool_id: &'a str,
    pub collateral_group_id: &'a str,
    pub reservation_id: &'a str,
}

pub enum ReservationRebuildEvidence<'a> {
    Fresh(&'a ReservationRequest),
    Retained(ExistingReservationIdentity<'a>),
}

pub struct ReservationLedgerCandidate {
    ledger: ReservationLedger,
    decision: CapitalAdmissionRebuildDecision,
}
```

`rollback_uncommitted` and `remove_existing` mutate exact existing rows even while unreconciled. New `reserve` and full-ledger replacement remain blocked until reconciliation is valid. Candidate construction never mutates `self`; accepted installation consumes the candidate.

- [ ] **Step 4: Write failing admission-state sequence tests**

Add behavior tests for:

```text
Reserved -> SinkInvoked -> omitted complete projection -> carried
SinkInvoked -> exact open -> ObservedOpen -> omitted complete projection -> carried
unreconciled + stale pool + exact terminal/zero leaves -> ledger and record retire together
stale candidate revision after terminal retirement -> candidate rejected, record not reinserted
incomplete/rejected/evidence-failed refresh -> ledger and record both preserved, new admission blocked
revision exhaustion -> prior state preserved and typed unhealthy failure returned
```

Assertions use real `BoltV3SubmitAdmissionState` behavior: exact live liability, phase-specific unresolved counts, reconciliation state, client-order presence, and returned decisions.

- [ ] **Step 5: Verify the admission tests fail**

```bash
cargo test --locked --features test-current-evidence-inspection --test admission_orders retained_lifecycle -- --test-threads=1
cargo test --locked --features test-current-evidence-inspection --test admission_orders terminal_reservation -- --test-threads=1
cargo test --locked --features test-current-evidence-inspection --test admission_orders stale_revision -- --test-threads=1
```

Expected: failures demonstrate current clearing, current epoch semantics, or missing phase/revision behavior.

- [ ] **Step 6: Implement typed records, revision ordering, and candidate swap**

Replace `BoltV3SubmitReservationIndex` with one record that contains immutable `ReservationAttribution`, fill metadata, exact phase, and `phase_revision`. Rename all epoch APIs and callers to state revision. Compute the checked successor before each mutation; on exhaustion preserve state, invalidate admission, and return a typed failure.

`commit_capital_admission_nt_projection` must:

```text
capture expected state revision
validate components and every NT open-order attribution into a candidate index
merge every absent SinkInvoked/ObservedOpen record through exact retained-ledger identity
build a candidate ledger without touching live state
append CapitalAdmissionRebuildV1 evidence
re-lock/recheck the exact revision
atomically swap candidate ledger + typed index + next revision
```

All invalidation paths call non-destructive `invalidate_reconciliation`; only initial construction uses an empty unreconciled gate. `retire_terminal_reservation` validates lifecycle identity in admission, appends the existing rebuild fact under the same lock, calls only generic `remove_existing`, then removes the typed record and installs the prepared revision.

- [ ] **Step 7: Make the focused capital tests green**

```bash
cargo test --locked --lib bolt_v3_capital_reservation::tests -- --nocapture
cargo test --locked --features test-current-evidence-inspection --test admission_orders -- --test-threads=1
```

Expected: all pass with carried numerical liability and no destructive failure path.

---

### Task 3: Cross one prepared submit boundary and retain every post-call outcome

**Files:**
- Modify: `src/bolt_v3_order_execution.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_order_execution/tracked_order_economics.rs`
- Modify: `src/bolt_v3_maker_order_dispatch.rs`
- Test: `src/bolt_v3_order_execution.rs`
- Test: `src/bolt_v3_order_execution/tracked_order_economics.rs`

**Interfaces:**
- Produces: `BoltV3SubmitAttemptKind::SinkInvokedUnknown` and matching state, replacing submit-only `SinkRejected`.
- Produces: `PreparedSubmitBoundary::{CapitalOnly, CapitalAndLifecycle}`.
- Produces: a fallible lifecycle preflight followed by an infallible `commit_sink_invoked` transition.
- Consumes: the prepared capital revision from Task 2.
- Test-only: `RouteBoundaryHarnessResult { outcome, participant_events, submit_calls, live_reserved_liability }` keeps assertions on real shared state rather than adding inspection methods to production outcomes.

- [ ] **Step 1: Change the existing sink-error tests to the required behavior**

The sink double must increment its submit counter and return an error. Change assertions so the result is `SinkInvokedUnknown`, capital liability remains positive, the typed reservation remains present, quote/resting participants receive a post-sink completion, and later route evaluation does not call the sink again for the same strategy claim.

- [ ] **Step 2: Add failing aggregate-boundary tests**

Add tests proving:

```rust
#[test]
fn prepared_boundary_preflight_failure_refunds_every_participant_and_skips_nt() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let result = route_boundary_harness(
        RecordingAttemptParticipant::failing_preflight(events.clone()),
        RecordingVenueMutationSink::default(),
    );

    assert_eq!(result.outcome.kind(), BoltV3SubmitAttemptKind::PreSinkRejected);
    assert_eq!(*events.borrow(), vec!["preflight", "pre_sink_unwind", "drop"]);
    assert_eq!(result.submit_calls, 0);
    assert_eq!(result.live_reserved_liability, Decimal::ZERO);
}

#[test]
fn prepared_boundary_commits_capital_and_lifecycle_before_one_direct_nt_call() {
    let events = Rc::new(RefCell::new(Vec::new()));
    let result = route_boundary_harness(
        RecordingAttemptParticipant::recording(events.clone()),
        RecordingVenueMutationSink::recording(events.clone()),
    );

    assert_eq!(result.outcome.kind(), BoltV3SubmitAttemptKind::Submitted);
    assert_eq!(
        *events.borrow(),
        vec!["capital_preflight", "lifecycle_preflight", "aggregate_commit", "nt_submit", "submitted", "drop"],
    );
}

#[test]
fn callback_retirement_between_nt_call_and_return_wins_over_sink_unknown_completion() {
    let result = resting_route_harness_with_reentrant_terminal_callback_and_sink_error();

    assert_eq!(result.route.kind(), BoltV3SubmitAttemptKind::SinkInvokedUnknown);
    assert_eq!(
        result.identity_disposition,
        RestingOrderIdentityDisposition::RetiredByCallback,
    );
    assert!(!result.registry_contains_exact_generation);
}
```

- [ ] **Step 3: Verify RED**

```bash
cargo test --locked --lib route_attempt_participant -- --nocapture
cargo test --locked --lib live_submit_failure -- --nocapture
```

Expected: current permit refunds and current participant settlement removes provisional registration.

- [ ] **Step 4: Implement the prepared aggregate**

Refactor the route protocol to the following ownership flow:

```rust
let prepared_capital = permit.prepare_sink_invocation()?;
let prepared_lifecycle = participant.map(|p| p.preflight(pre_sink_now_ns)).transpose()?;
let committed = PreparedSubmitBoundary::new(prepared_capital, prepared_lifecycle)
    .commit_sink_invoked(); // infallible, no allocation, lookup, clock, evidence, or lock acquisition
let nt_result = sink.submit_order_via_nt(order, context);
committed.complete(nt_result); // Submitted or SinkInvokedUnknown; neither refunds
```

No guard remains held across the NT call. Every fallible lifecycle operation moves into preflight; if that cannot be encoded, fail before NT rather than allowing a mixed boundary.

- [ ] **Step 5: Make shared submit and resting tests green**

```bash
cargo test --locked --lib route_attempt_participant -- --nocapture
cargo test --locked --lib resting_registration -- --nocapture
cargo test --locked --lib live_submit_failure -- --nocapture
```

Expected: post-call errors retain capital, quote liability, and exact registration; preflight errors unwind all.

---

### Task 4: Return exact maker identity and cancellation dispositions

**Files:**
- Modify: `src/bolt_v3_order_execution/tracked_order_economics.rs`
- Modify: `src/bolt_v3_order_execution/tracked_order_economics/cancel_coordinator.rs`
- Modify: `src/bolt_v3_maker_order_dispatch.rs`
- Modify: `src/strategies/binary_oracle_maker/runtime.rs`
- Modify: affected maker test helpers in `src/bolt_v3_order_execution.rs`
- Test: `src/bolt_v3_order_execution/tracked_order_economics.rs`
- Test: `tests/bolt_v3_binary_oracle_maker_runtime.rs`
- Test: `tests/bolt_v3_maker_order_dispatch.rs`

**Interfaces:**
- Produces: `RestingOrderIdentityDisposition::{RetainedActive, RetiredByCallback, NotRetained}`.
- Produces: handled cancel results carrying exact per-client-order coordinator dispositions.
- Replaces: `BoltV3RestingSubmitTransactionOutcome::is_submitted`, `MakerOrderDispatchOutcome::Canceled`, and `CanceledAll` as identity authority.

- [ ] **Step 1: Add failing synchronous-retirement and binding tests**

For both `Submitted` and `SinkInvokedUnknown`, synchronously retire the exact provisional generation before the sink returns. Assert `RetiredByCallback` and no active maker binding. Assert only `RetainedActive` promotes `next_order`.

- [ ] **Step 2: Add failing maker cancellation disposition tests**

Assert `CancelIntentHandled` and `CancelScopeHandled` clear only identities whose coordinator disposition is accounted for. Missing or mismatched active identity preserves the binding and poisons registry health. An unrouted `next_order` may be cleared. No handled result is asserted as NT-route or terminal proof.

- [ ] **Step 3: Verify RED**

```bash
cargo test --locked --lib synchronous_terminal_callback -- --nocapture
cargo test --locked --features test-current-evidence-inspection --test maker_taker cancel_intent_handled -- --test-threads=1
```

Expected: the current Boolean promotion resurrects identity or the current cancel outcome clears without a disposition.

- [ ] **Step 4: Implement exact dispositions**

Make resting settlement return the exact generation disposition and exclude `SinkInvokedUnknown` from `BoltV3RoutedNonSubmittedOutcome`. Thread the coordinator's existing selected IDs and terminal/retained status through `route_tracked_cancel` and `route_tracked_cancel_all` rather than discarding them. Rename dispatch outcomes and make `rotate_leg_identity` exhaustive over the new dispositions.

- [ ] **Step 5: Make maker tests green**

```bash
cargo test --locked --lib tracked_order_economics -- --nocapture
cargo test --locked --features test-current-evidence-inspection --test maker_taker -- --test-threads=1
```

---

### Task 5: Replace edge-taker projections and recovery with one private owner

**Files:**
- Rewrite: `src/strategies/binary_oracle_edge_taker/exposure.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/tests/{exposure.rs,adverse_path_harness.rs,core_glue.rs,shared_fixture.rs}`
- Modify: `src/bolt_v3_order_execution.rs`
- Modify: `src/bolt_v3_settlement_booking.rs`

**Interfaces:**
- Produces: private `ExposureOwner` containing private `ExposureState`.
- Produces: `EntryRemainder { pending_entry, position, cancellation }` with `EntryRemainderPosition::{Supported, Unsupported, CanonicallyFlat}` and `EntryCancellation::{Working, Pending, Refused}`.
- Produces: non-cloneable exact-generation entry and exit/cancel settlement capabilities.
- Deletes: recovered exit authority, restart adoption, recovery hold, `ManagedPositionOrigin`, optional managed pending entry, flat-terminal override, fill-void reconstruction, and cache-absence release.

- [ ] **Step 1: Write failing owner-level behavior tests**

Test the private public-to-strategy API rather than enum text:

```text
arm_entry succeeds only from Flat and installs identity before routing
exact pre-sink abort releases only the still-armed generation
synchronous terminal observation wins over Submitted/SinkInvokedUnknown settlement
partial fill with a persistent order becomes EntryRemainder and stays occupied
supported/unsupported/canonically-flat remainder each requests at most one cancel
policy skip restores exact Working; NT return/error stays Pending
cancel rejection becomes idempotent Refused and no exit/retry occurs
terminal/zero-leaves plus coherent truth releases to Managed/Unsupported/Flat
```

Name the production mutation each test catches in its test name or adjacent comment.

- [ ] **Step 2: Replace reconstruction tests with non-routing sequences**

Change existing restart, fill-void, and cache-miss tests so any cached open position or unlineaged live position enters `BlindRecovery`; later position/cache events cannot create `Managed`, `ExitPending`, or `Flat`. Fill void after local authority retirement also stays blind and produces zero route calls.

- [ ] **Step 3: Verify RED**

```bash
cargo test --locked --features test-current-evidence-inspection --lib binary_oracle_edge_taker::tests::exposure -- --test-threads=1
cargo test --locked --features test-current-evidence-inspection --lib binary_oracle_edge_taker::tests::adverse_path_harness -- --test-threads=1
```

Expected: current restart adoption, forced-flat bypass, optional pending-entry loss, and fill-void reconstruction violate the new expectations.

- [ ] **Step 4: Implement the private owner and exhaustive reducers**

`BinaryOracleEdgeTaker` stores `ExposureOwner`, never `ExposureState`. Orchestration may call only typed query/command methods such as `occupancy`, `entry_block_reason`, `arm_entry`, `apply_observation`, `request_exit`, and capability settlement; it cannot construct, assign, or match inner variants.

One release reducer is the sole runtime producer of `Flat`. It takes typed observations for entry terminal, position truth, position close, settlement, exit terminal, cancel rejection, and exact pre-sink entry abort. Every authority-mutating match enumerates all variants without `_`. `Flat` is never a `mem::replace` sentinel.

- [ ] **Step 5: Delete strategy-owned recovery surfaces**

Delete every production symbol listed by the spec, including the corresponding shared recovered-exit types when no longer used. Retain only local exit authority. Split timer reconciliation so exact present working/terminal observations remain while cache absence only records evidence and preserves state.

- [ ] **Step 6: Make all edge behavior tests green**

```bash
cargo test --locked --features test-current-evidence-inspection --lib binary_oracle_edge_taker::tests -- --test-threads=1
```

Expected: all edge tests pass with no reconstructed route authority and no second cancel/submit path.

---

### Task 6: Cut over current evidence for phase-specific unresolved reservations

**Files:**
- Modify: `src/bolt_v3_current_evidence/facts.rs`
- Modify: `src/bolt_v3_current_evidence/codec/admission.rs`
- Modify: `src/bolt_v3_current_evidence/codec.rs`
- Regenerate: `src/bolt_v3_current_evidence/generated_contract.rs`
- Modify: `config/decision-evidence-contract.toml`
- Modify: `tests/fixtures/bolt_v3/current_evidence/positive/capital_admission_rebuild.jsonl`
- Modify: relevant accepted/rejection current-evidence fixtures if regeneration changes them
- Test: `src/bolt_v3_current_evidence/codec.rs`
- Test: `tests/bolt_v3_current_evidence_contract.rs`
- Test: `tests/bolt_v3_current_evidence_runtime.rs`

**Interfaces:**
- Extends: `CapitalAdmissionRebuildFact` with `unresolved_sink_invoked_reservation_count` and `unresolved_observed_open_reservation_count`.
- Derives: checked `unresolved_lifecycle_reservation_count`; it is not a third wire field.
- Preserves: `CapitalAdmissionRebuildV1` as the sole current identity while bumping its configured schema version from 16 to 17.

- [ ] **Step 1: Write failing codec/domain tests**

Add round-trip and rejection tests for both counts, overflow of their checked sum, and unknown/absent fields under the current-only schema. Update the positive fixture expectation; do not add a compatibility decoder.

- [ ] **Step 2: Verify RED**

```bash
cargo test --locked --lib capital_rebuild -- --nocapture
cargo test --locked --test bolt_v3_current_evidence_contract -- --test-threads=1
cargo test --locked --test bolt_v3_current_evidence_runtime -- --test-threads=1
```

- [ ] **Step 3: Implement and regenerate the current contract**

Update the fact and `CapitalAdmissionRebuildV1` wire, validate both counts and their checked sum, bump only the configured current identity, and run:

```bash
cargo run --locked --bin generate_decision_evidence_contract
```

Regenerate the current positive/rejection corpus through the existing generator or fixture workflow; do not hand-create an alternate format.

- [ ] **Step 4: Make evidence tests green**

```bash
cargo test --locked --lib capital_rebuild -- --nocapture
cargo test --locked --test bolt_v3_current_evidence_contract -- --test-threads=1
cargo test --locked --test bolt_v3_current_evidence_runtime -- --test-threads=1
```

---

### Task 7: Integrate the atomic behavior cutover

**Files:**
- Modify: every production and behavior-test path changed in Tasks 2–6.
- Modify: `docs/superpowers/plans/2026-08-19-pr-1544-lifecycle-first-scope-back.md` with exact evidence.

**Interfaces:**
- Consumes: all green focused task states in the same working tree.
- Produces: one compile-complete production authority graph with no intermediate dual path.

- [ ] **Step 1: Run compile and focused integration gates**

```bash
cargo check --locked --bin bolt-v2
cargo test --locked --features test-current-evidence-inspection --lib binary_oracle_edge_taker::tests -- --test-threads=1
cargo test --locked --features test-current-evidence-inspection --test admission_orders -- --test-threads=1
cargo test --locked --features test-current-evidence-inspection --test maker_taker -- --test-threads=1
```

- [ ] **Step 2: Inspect the authority graph**

Use targeted `rg` to confirm no production occurrence remains for recovered-exit authority, recovery holds, `ManagedPositionOrigin`, `SinkRejected` submit classification, direct orchestration assignment to exposure state, destructive current-process gate replacement, or maker `Canceled`/`CanceledAll` authority names. Confirm shared modules contain no edge-taker type or strategy-name reference.

- [ ] **Step 3: Commit the one production behavior cutover**

Stage the complete production/test/evidence delta together and inspect it before committing:

```bash
git diff --cached --stat
git diff --cached --check
git commit -m "refactor(economics): follow submit lifecycle authority"
```

No partial production commit from Tasks 2–6 is permitted.

---

### Task 8: Verify debt reduction and exact-head behavior

**Files:**
- Create outside the repository: `/tmp/bolt-v2-pr1544-conditional-census.py`
- Create outside the repository: `/tmp/bolt-v2-pr1544-conditional-census-output.txt`
- Modify: implementation plan checkboxes/evidence only if the working tree remains reviewable.

**Interfaces:**
- Consumes: exact committed implementation head.
- Produces: reproducible exact-head behavior, static, and conditional-debt evidence.

- [ ] **Step 1: Run formatting and static checks**

```bash
cargo fmt --all --check
git diff --check 23960a0dcf4232c818db4b539a41ac5b4bb928d7..HEAD
git diff --check e62584045629208e81d2dce1fce608720ea01fbf..HEAD
```

- [ ] **Step 2: Run focused lint and behavior checks**

```bash
cargo clippy --locked --bin bolt-v2 -- -D warnings
cargo test --locked --features test-current-evidence-inspection --lib binary_oracle_edge_taker::tests -- --test-threads=1
cargo test --locked --features test-current-evidence-inspection --test admission_orders -- --test-threads=1
cargo test --locked --features test-current-evidence-inspection --test maker_taker -- --test-threads=1
cargo test --locked --test bolt_v3_current_evidence_contract -- --test-threads=1
cargo test --locked --test bolt_v3_current_evidence_runtime -- --test-threads=1
```

- [ ] **Step 3: Run isolated backtesting checks when the shared API is imported there**

```bash
cd crates/backtesting-vertical-slice
cargo fmt --check
cargo clippy --locked --lib --bins -- -D warnings
```

- [ ] **Step 4: Run the exact conditional census**

The scanner must strip Rust comments and string/character literals, exclude test paths and complete `#[cfg(test)]` items, and report per-file added/removed/net values for primary lines, match arms, alternate conditional lines, and the companion union over both immutable ranges. Record the scanner SHA-256, invocation, and raw output.

Stop for an explicit scope decision if either repair-range strategy budget is non-negative or the complete-PR primary result is `+250` or greater. Do not move reducers, rewrite syntax, or touch unrelated code to game the result.

- [ ] **Step 5: Conduct the required internal adversarial review**

Re-read the full design against the exact diff. Trace at least these sequences: post-call submit error; synchronous terminal callback; omitted `SinkInvoked`; omitted `ObservedOpen`; unreconciled terminal retirement; stale candidate; restart open position; persistent partial-fill remainder; cancel rejection replay; maker synchronous retirement; maker cancel missing identity. Every unique substantive issue is fixed before external review.

---

### Task 9: Publish exact-head review evidence without changing merge or live state

**Files:**
- Modify externally: stable PR #1544 body only for lasting lifecycle-first scope and unchanged #869/Slices 2–5 remainder.
- Do not put transient head/check receipts in the PR body.

**Interfaces:**
- Produces: pushed exact implementation head and review request; no merge, deploy, readiness, or trading authority.

- [ ] **Step 1: Push the exact branch head**

```bash
git status --short --branch
git push
```

- [ ] **Step 2: Update lasting PR disclosure and request review**

Resolve applicable review threads, preserve the stable body, request the required native code-owner review, and provide external reviewers the exact base/head, raw verification commands, census artifacts, scope remainder, and explicit no-merge/no-live boundary.

- [ ] **Step 3: Stop after reporting the pushed SHA**

Do not wait on advisory CI and do not merge. Exact-head CI/reviewer evidence is adjudicated in the next review round.
