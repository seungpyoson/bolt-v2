# Economics Slice 1 External-Review Repairs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` inline. Do not delegate: the user explicitly prohibited Codex delegation. Track every step with the checkboxes below.

**Goal:** Resolve every substantive Claude and GPT finding on PR #1544 with compiler-shaped authorities, fail-closed behavior, fewer duplicated conditional paths, and corrected lasting review evidence.

**Architecture:** Exposure recovery, maker command binding, quote transactions, budget liabilities, and submit scenarios each receive one typed authority boundary. Runtime-invalid correlated values are replaced by enums that own their required payloads; duplicated reducers are collapsed behind one exhaustive transition. Historical forced-reduction evidence remains decodable but cannot create current submit or recovery authority.

**Tech Stack:** Rust 2024, NautilusTrader Rust API pinned by `Cargo.lock`, `rust_decimal`, Serde/TOML, existing Bolt current-evidence codecs, Cargo/nextest, GitHub advisory CI.

**Spec:** `docs/superpowers/specs/2026-08-18-economics-slice-1-external-review-repairs-design.md`

## Global Constraints

- The reviewed production base is `524362b68ed86d7bc84f63655b8202590974dac9`; preserve unrelated work and never consult Bolt v1.
- Work inline in `.worktrees/1445-economics-cutover`; do not spawn Codex subagents.
- Do not add source-scanning or source-structure tests. Use behavior tests and compiler exhaustiveness.
- Do not add `panic!`, `assert!`, or `unreachable!` to a production path for a constructible runtime state.
- Do not use wildcard arms over `ExposureState`, quote-transaction phases, maker legs, submit intents, or liability phases.
- Missing fee authority, stale generation, malformed leg authority, and recovery payloads fail before mutation.
- An NT mutation invocation remains conservatively charged even when its synchronous result is `Err`.
- Keep historical forced-reduction codecs readable, but remove all current producers, submit permits, recovery authorizations, and execution scenarios.
- Use the repository-configured `/Volumes/CargoBuild/bolt-v2` target and `CARGO_BUILD_JOBS=2` for focused local Cargo commands. The `/Volumes/T9/bolt-v2-target-1544-review-repairs` cache does not support incremental hard links; use it only with `CARGO_INCREMENTAL=0` when exact comparison with prior review artifacts is required. Run Cargo commands sequentially and append `-- --test-threads=1` to tests.
- During implementation, run the smallest named tests. At final head run formatting/static checks and rely on advisory CI for the compile-heavy workspace evidence; do not wait on CI.
- Commit each task only after its named checks pass. Do not push with unresolved findings or uncommitted changes.
- Never merge. A merge still requires explicit user authorization and approval from reviewer node `U_kgDOEZMFhA` (currently `sp-reviewer`).

## File map

- `src/strategies/binary_oracle_edge_taker/exposure.rs`: sole exposure reducer, cause-shaped recovery, exhaustive projections, operation classification.
- `src/strategies/binary_oracle_edge_taker/{entry_decision.rs,exit_decision.rs,mod.rs}`: decision-bound generation and exact rejection evidence.
- `src/strategies/binary_oracle_edge_taker/tests/exposure.rs`: exposure ordering and operation-fence behavior tests.
- `src/bolt_v3_current_evidence/{facts.rs,codec/entry_skip.rs}`: new typed entry-operation skip reasons.
- `src/bolt_v3_providers/polymarket/economics.rs`: provider-only fee authority.
- `tests/fixtures/economics/polymarket/explicit_zero_fee.json`: synthetic explicit-zero provider descriptor.
- `tests/support/economics.rs`, `tests/bolt_v3_economics_runtime.rs`, and four TOML roots/profiles: migrate away from the deleted absent-descriptor policy.
- `src/bolt_v3_maker_order_dispatch.rs`, `src/strategies/binary_oracle_maker/mod.rs`, and maker tests: sealed leg/instrument/proposal authority.
- `src/bolt_v3_quote_lifecycle.rs`: shared attempt phase, non-recursive settlement, and truthful NT mutation naming.
- `src/bolt_v3_requote_budget.rs`: phase-shaped outstanding liabilities.
- `src/bolt_v3_order_execution/tracked_order_economics.rs` and `cancel_coordinator.rs`: terminal refinement, actor time, and conservative NT invocation settlement.
- `src/bolt_v3_order_execution/{economics_basis.rs}` and `src/{bolt_v3_order_execution.rs,bolt_v3_submit_admission.rs}`: delete current forced-reduction economics/admission authority.
- `src/bolt_v3_current_evidence/{reader.rs,record.rs,handles.rs,facts.rs}`: retain decoding while removing production and recovery authority.
- `src/bolt_v3_kill_switch_{flatten,action_router}.rs`: retain proof-only planning types outside submit admission.
- `src/bolt_v3_live_node.rs` and affected admission/evidence tests: remove forced-reduction liveness and current authorization plumbing.
- PR #1544 and issue #869: lasting scope disclosure and deferred maker event-fence record.

---

### Task 1: Give replacement conflict one discharge transition

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/exposure.rs:1747-1810,2344-2385,834-879,3340-3397`
- Test: `src/strategies/binary_oracle_edge_taker/tests/exposure.rs:4450-4910`

**Interfaces:**
- Produces: `ReplacementConflictState::observe_projection`, `ReplacementConflictState::resolve`, and `ReplacementConflictResolution { state, adoption }`.
- Consumes: existing `ReplacementCandidateProjection`, `FreshCanonicalPositionProjection`, and `ReplacementAdoption`.

- [x] **Step 1: Add the close-then-projection regression test**

Add a fixture that creates a partially filled position with a working entry remainder, then enters `ReplacementConflict`:

```rust
fn replacement_conflict_with_working_remainder(
    strategy: &mut BinaryOracleEdgeTaker,
) -> (PositionEpisodeFingerprint, PendingEntryState) {
    let instrument_id = selected_entry_instrument(strategy);
    materialize_managed_position_with_resting_pending_entry(
        strategy,
        instrument_id,
        PositionId::from("P-RETAINED-WITH-REMAINDER"),
        Quantity::new(5.0, 2),
    );
    let retained = strategy
        .exposure
        .managed_position_context()
        .expect("retained position should be managed")
        .clone();
    let pending = retained
        .pending_entry
        .clone()
        .expect("partial entry remainder should still be working");
    let mut candidate = retained.clone();
    candidate.position_id = PositionId::from("P-REPLACEMENT-CANDIDATE");
    candidate.episode.position_id = candidate.position_id;
    candidate.episode.opening_order_id = ClientOrderId::from("ENTRY-REPLACEMENT-CANDIDATE");
    candidate.episode.ts_opened_ns = 2_000;
    strategy.exposure.reduce_without_replacement_adoption(
        AdoptionCapableExposureEvent::PositionTruth(
            AdoptionCapablePositionTruthEvent::Canonical(
                CanonicalPositionProjection::ExactlyOne(Box::new(candidate)),
            ),
        ),
    );
    (retained.episode, pending)
}
```

Add `replacement_conflict_close_then_canonical_none_preserves_working_remainder`: close the retained episode with `FreshCanonicalPositionProjection::ProbeFailed`, then apply `CanonicalPositionProjection::None`. Assert `ExposureState::PendingEntry` retains the exact client order ID and `request_entry_operation` returns `PendingEntryOccupied`.

- [x] **Step 2: Run the regression and verify the current head fails**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib replacement_conflict_close_then_canonical_none_preserves_working_remainder -- --test-threads=1
```

Expected: FAIL because the later canonical `None` changes the conflict to `Flat`.

- [x] **Step 3: Add the reverse-order behavior test**

Add `replacement_conflict_canonical_none_then_close_preserves_working_remainder`: apply canonical `None` first, close with fresh `None` second, and assert the identical `PendingEntry` state and denied new entry grant.

- [x] **Step 4: Implement the single discharge owner**

Move projection storage and discharge into `ReplacementConflictState`:

```rust
#[derive(Debug)]
struct ReplacementConflictResolution {
    state: ExposureState,
    adoption: Option<ReplacementAdoption>,
}

impl ReplacementConflictState {
    fn observe_projection(&mut self, projection: ReplacementCandidateProjection) {
        self.candidate_projection = projection;
    }

    fn resolve(self) -> ReplacementConflictResolution {
        if !self.retained_is_closed() {
            return ReplacementConflictResolution {
                state: ExposureState::ReplacementConflict(Box::new(self)),
                adoption: None,
            };
        }
        match self.candidate_projection {
            ReplacementCandidateProjection::Matching => {
                let adoption = ReplacementAdoption {
                    retained_episode: self.retained.episode.clone(),
                    adopted: self.candidate.clone(),
                    cause: ReplacementAdoptionCause::CanonicalCloseConjunction,
                };
                ReplacementConflictResolution {
                    state: ExposureState::Managed(self.candidate),
                    adoption: Some(adoption),
                }
            }
            ReplacementCandidateProjection::None => ReplacementConflictResolution {
                state: self
                    .retained
                    .pending_entry
                    .clone()
                    .map_or(ExposureState::Flat, ExposureState::PendingEntry),
                adoption: None,
            },
            ReplacementCandidateProjection::Divergent { .. }
            | ReplacementCandidateProjection::Multiple { .. }
            | ReplacementCandidateProjection::ProbeFailed { .. } => {
                ReplacementConflictResolution {
                    state: ExposureState::ReplacementConflict(Box::new(self)),
                    adoption: None,
                }
            }
        }
    }
}
```

Both `reduce_canonical_projection` and `reduce_position_closed` must update the conflict and call `resolve`; delete `resolve_replacement_close` so no second discharge policy remains.

- [x] **Step 5: Run the replacement-conflict test slice**

Run:

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib replacement_conflict -- --test-threads=1
```

Expected: PASS, including both event orderings and candidate adoption.

- [x] **Step 6: Commit**

```bash
git add src/strategies/binary_oracle_edge_taker/exposure.rs src/strategies/binary_oracle_edge_taker/tests/exposure.rs
git commit -m "fix(exposure): preserve working entry through replacement conflict"
```

### Task 2: Make blind recovery payloads and exposure projections exhaustive

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/exposure.rs:520-770,2800-2920,4160-4320`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs` at every `BlindRecoveryState` constructor and pattern
- Test: `src/strategies/binary_oracle_edge_taker/tests/exposure.rs:3170-3505,5220-5410`
- Test fixtures: `src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs`

**Interfaces:**
- Produces: `BlindRecoveryCause`, `BlindRecoveryAuthority`, `NonEmptyRestartOrderIds`, and `ExposureProjection<'a>`.
- Preserves: `BlindRecoveryState::reason()`, retained-authority behavior, and existing query method return types.

- [x] **Step 1: Add cause/authority behavior coverage**

Replace the current invalid-combination matrix with behavior cases for each constructible class:

```rust
let cases = [
    BlindRecoveryState::probe(BlindRecoveryProbeReason::CacheProbeFailed),
    BlindRecoveryState::identity_bearing(
        BlindRecoveryIdentityReason::DivergentUnsupported,
        recorded_episode.clone(),
    ),
    BlindRecoveryState::restart_adoption(
        BlindRecoveryRestartReason::UnattributedOpenExitOrder,
        instrument_id,
        first_order_id,
        remaining_order_ids,
    ),
    BlindRecoveryState::foreign_venue(instrument_id, instrument_venue, execution_venue),
];
```

For every case, assert the derived `BlindRecoveryReason`, authority-free release rule, and behavior after `retain_authority`. Do not add a test that calls an invalid constructor; those constructors will not exist.

- [x] **Step 2: Run the blind-recovery tests as a baseline**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib blind_recovery -- --test-threads=1
```

Expected before the refactor: existing behavior tests pass; the new constructor names fail to compile until Step 3.

- [x] **Step 3: Replace reason/provenance correlation with cause variants**

Use payload-owning types:

```rust
#[derive(Debug, Clone, PartialEq)]
enum BlindRecoveryAuthority {
    AuthorityFree,
    Retained(Box<ExposureState>),
}

#[derive(Debug, Clone, PartialEq)]
struct NonEmptyRestartOrderIds {
    first: ClientOrderId,
    remaining: Vec<ClientOrderId>,
}

#[derive(Debug, Clone, PartialEq)]
enum BlindRecoveryCause {
    Probe(BlindRecoveryProbeReason),
    IdentityBearing {
        reason: BlindRecoveryIdentityReason,
        recorded_episode: PositionEpisodeFingerprint,
    },
    RestartAdoption {
        reason: BlindRecoveryRestartReason,
        instrument_id: InstrumentId,
        order_ids: NonEmptyRestartOrderIds,
    },
    ForeignVenue {
        instrument_id: InstrumentId,
        instrument_venue: Venue,
        execution_venue: Venue,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct BlindRecoveryState {
    cause: BlindRecoveryCause,
    authority: BlindRecoveryAuthority,
}
```

Construct restart recovery with `(first_order_id, remaining_order_ids)` so empty identity sets are unrepresentable. Derive the evidence-facing `BlindRecoveryReason` in one exhaustive `reason()` match. Store the orthogonal authority once on `BlindRecoveryState`, implement it through `BlindRecoveryAuthority`, and delete `authority_free(reason)`, `restart_adoption(reason, ...)`, and every reason/provenance assertion or panic.

- [x] **Step 4: Add one exhaustive exposure projection**

Create a borrowed view and one exhaustive match:

```rust
struct ExposureProjection<'a> {
    pending_entry: Option<&'a PendingEntryState>,
    managed: Option<&'a ManagedPositionContext>,
    tracked: Option<&'a ManagedPositionContext>,
    exit: Option<ExitProjection<'a>>,
    recovery_hold: Option<&'a ExitAuthorityRecoveryHoldState>,
    sink_unknown: Option<&'a OperationSinkUnknownState>,
    occupancy: Option<ExposureOccupancy>,
}

enum ExitProjection<'a> {
    Attempting(&'a ExitAttemptingState),
    Working(&'a ExitPendingState),
    TerminalAwaitingPosition(&'a ExitPendingState),
}
```

`ExposureState::projection()` must explicitly name all 13 variants. `BlindRecovery` and `ObligationSaturated` recursively project retained authority, then override wrapper occupancy. Convert `pending_entry`, `managed_position_context`, `tracked_position_context`, `exit_pending_snapshot`, `exit_lifecycle`, `exit_authority_recovery_hold`, `operation_sink_unknown`, and `occupancy` to projection accessors. Remove their independent matches and all six `_ => None` arms.

- [x] **Step 5: Migrate constructors and patterns**

Update every production/test call site to the cause-specific constructor. Where restart discovery currently owns `Vec<ClientOrderId>`, split it before construction:

```rust
let mut order_ids = observed_order_ids.into_iter();
let first_order_id = order_ids
    .next()
    .ok_or_else(|| anyhow::anyhow!("restart recovery requires an observed order identity"))?;
let recovery = BlindRecoveryState::restart_adoption(
    reason,
    instrument_id,
    first_order_id,
    order_ids.collect(),
);
```

The error is returned at the discovery boundary; no recovery constructor asserts.

- [x] **Step 6: Run exposure and formatting checks**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib binary_oracle_edge_taker::tests::exposure -- --test-threads=1
cargo fmt --all -- --check
```

Expected: PASS and compiler-exhaustive projection/cause matches.

- [x] **Step 7: Commit**

```bash
git add src/strategies/binary_oracle_edge_taker/exposure.rs src/strategies/binary_oracle_edge_taker/mod.rs src/strategies/binary_oracle_edge_taker/tests/exposure.rs src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs
git commit -m "refactor(exposure): type recovery causes and projections"
```

### Task 3: Bind entry and exit decisions to one exposure generation

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/exposure.rs:2860-3020`
- Modify: `src/strategies/binary_oracle_edge_taker/{entry_decision.rs,exit_decision.rs,mod.rs}`
- Modify: `src/bolt_v3_current_evidence/facts.rs:660-690`
- Modify: `src/bolt_v3_current_evidence/codec/entry_skip.rs`
- Modify: `config/evidence-novelty.toml`
- Test: `src/strategies/binary_oracle_edge_taker/tests/{exposure.rs,source_evidence.rs}`
- Test: current-evidence codec unit tests in `src/bolt_v3_current_evidence/codec.rs`

**Interfaces:**
- Produces: `ExposureOperationDecision { generation, rejection }`, `operation_generation` on entry/exit decisions, and entry skip variants `EntryOperationStaleGeneration` and `EntryOperationAlreadyArmed`.
- Consumes: existing `ExposureOperationBlockedReason` and `request_{entry,exit}_operation(expected_generation)`.

- [x] **Step 1: Write stale and already-armed entry-route tests**

Add two tests. The stale case creates an admitted entry decision, transitions `Flat -> PendingEntry -> Flat`, then routes the old decision. The already-armed case holds an entry grant while routing the decision. Assert no sink/tracked order mutation and exact evidence:

```rust
assert_eq!(
    recorded_skip.reason_category,
    EntrySkipReason::EntryOperationStaleGeneration,
);
assert_eq!(
    armed_skip.reason_category,
    EntrySkipReason::EntryOperationAlreadyArmed,
);
```

- [x] **Step 2: Run the new tests and verify failure**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib entry_operation_ -- --test-threads=1
```

Expected: FAIL because decisions carry no generation and all route rejections are labeled one-position violations.

- [x] **Step 3: Centralize pure operation classification**

Implement one classifier used by both decision inspection and grant construction:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ExposureOperationDecision {
    pub(super) generation: u64,
    pub(super) rejection: Option<ExposureOperationBlockedReason>,
}

fn classify_operation(
    inner: &GovernedExposureInner,
    operation: ExposureOperationKind,
    expected_generation: u64,
) -> ExposureOperationDecision {
    let rejection = match (
        inner.operation_arm.is_some(),
        expected_generation == inner.generation,
    ) {
        (true, _) => Some(ExposureOperationBlockedReason::OperationAlreadyArmed),
        (false, false) => Some(ExposureOperationBlockedReason::StaleGeneration),
        (false, true) => (!inner.state.allows_operation(operation))
            .then(|| blocked_reason_for_state(&inner.state)),
    };
    ExposureOperationDecision {
        generation: expected_generation,
        rejection,
    }
}
```

The existing exhaustive exposure projection owns an `ExposureOperationPermissions` value whose `(permissions, operation)` table names every combination. `inspect_operation` calls the classifier without mutation; `request_operation` calls the same classifier and arms only an allowed exact generation. An occupied sole arm takes precedence over the generation it advanced, so overlapping requests are classified as `OperationAlreadyArmed`; an unarmed changed generation is `StaleGeneration`.

- [x] **Step 4: Carry generation through entry and exit decisions**

Add `operation_generation: u64` to `EntrySubmissionDecision` and `ExitEvaluation`. At evaluation, call `inspect_entry_operation` or `inspect_exit_operation`. Delete the exit probe-grant/drop cycle. At route time, pass `decision.operation_generation`; never refresh it with `self.exposure.generation()`.

- [x] **Step 5: Map exact entry rejections and remove the panic path**

Add:

```rust
const fn entry_operation_blocked_reason(
    reason: ExposureOperationBlockedReason,
) -> EvidenceEntrySkipReason {
    match reason {
        ExposureOperationBlockedReason::StaleGeneration => {
            EvidenceEntrySkipReason::EntryOperationStaleGeneration
        }
        ExposureOperationBlockedReason::OperationAlreadyArmed => {
            EvidenceEntrySkipReason::EntryOperationAlreadyArmed
        }
        ExposureOperationBlockedReason::Unoccupied
        | ExposureOperationBlockedReason::PendingEntryOccupied
        | ExposureOperationBlockedReason::EntryReconcileOccupied
        | ExposureOperationBlockedReason::ManagedOccupied
        | ExposureOperationBlockedReason::ExitAttemptOccupied
        | ExposureOperationBlockedReason::ExitPendingOccupied
        | ExposureOperationBlockedReason::RecoveryHoldOccupied
        | ExposureOperationBlockedReason::UnsupportedOccupied
        | ExposureOperationBlockedReason::BlindRecoveryOccupied
        | ExposureOperationBlockedReason::SinkUnknownOccupied
        | ExposureOperationBlockedReason::ReplacementConflictOccupied
        | ExposureOperationBlockedReason::ObligationSaturated => {
            EvidenceEntrySkipReason::OnePositionInvariantViolation
        }
    }
}
```

Record the mapped reason and return `Ok(None)`. Delete the second occupancy check and `unreachable!`.

- [x] **Step 6: Extend evidence codecs exhaustively**

Add both new `EntrySkipReason` variants to `facts.rs`, `codec/entry_skip.rs`, canonical-state/label matches in `entry_decision.rs`, and round-trip tests. Do not change historical enum spellings.

- [x] **Step 7: Run focused behavior and codec tests**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib entry_operation_ -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib entry_skip -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib exit_route_grant -- --test-threads=1
```

Expected: PASS; stale/armed entry decisions do not panic or route.

- [x] **Step 8: Commit**

```bash
git add src/strategies/binary_oracle_edge_taker src/bolt_v3_current_evidence/facts.rs src/bolt_v3_current_evidence/codec/entry_skip.rs src/bolt_v3_current_evidence/codec.rs config/evidence-novelty.toml
git commit -m "fix(exposure): bind decisions to operation generations"
```

### Task 4: Make Polymarket fee descriptor absence fail closed

**Files:**
- Modify: `src/bolt_v3_providers/polymarket/economics.rs`
- Create: `tests/fixtures/economics/polymarket/explicit_zero_fee.json`
- Modify: `tests/support/economics.rs`
- Modify: `tests/bolt_v3_economics_runtime.rs`
- Modify: `config/root.toml`
- Modify: `config/profiles/prod-btc-5m.overlay.toml`
- Modify: `tests/fixtures/bolt_v3/root.toml`
- Modify: `tests/fixtures/legacy_prod_btc_5m_oracle.toml`

**Interfaces:**
- Removes: `PolymarketAbsentFeeDescriptorPolicy` and `PolymarketEconomicsConfig::absent_fee_descriptor_policy`.
- Preserves: explicit provider `fd.r == 0` as provider-sourced `PointEstimate::ProvenZero`.

- [x] **Step 1: Replace the assertion test with two authority tests**

Rename the descriptor-absence test to `absent_fee_descriptor_always_fails_closed` and delete its asserted-zero branch. Add `explicit_zero_fee_descriptor_is_provider_sourced_proven_zero`, using a fixture whose only semantic difference is:

```json
"fd": { "r": 0, "e": 1, "to": true }
```

Assert one component, `PointEstimate::ProvenZero`, and source equality with the provider snapshot metadata.

- [x] **Step 2: Run the explicit-zero test and verify it initially lacks a fixture/path**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib explicit_zero_fee_descriptor_is_provider_sourced_proven_zero -- --test-threads=1
```

Expected: FAIL until the explicit descriptor fixture is created and call sites stop using descriptor absence as zero.

- [x] **Step 3: Delete the config policy and unconditionalize fail-closed behavior**

Remove the policy enum/field, formula-key validation, config build match, and TOML key from all four config files. Replace the quote branch with:

```rust
if self.snapshot.platform == PolymarketPlatformPlan::FeeDescriptorUnknown {
    return Err(PolymarketEconomicsError::FeeDescriptorUnknown);
}
```

Keep `effect` unchanged so an explicit evaluated zero descriptor remains the only route to `ProvenZero`.

- [x] **Step 4: Migrate positive fixtures to explicit zero**

Create `explicit_zero_fee.json` from the descriptor-missing fixture plus the explicit `fd` object. Keep `fee_free.json` descriptor-missing and negative. Update `tests/support/economics.rs` and `fee_free_authoritative_input` to use `explicit_zero_fee.json`.

- [x] **Step 5: Run fee, config, and routed-economics tests**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib polymarket::economics -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test wiring_registration bound_execution_economics -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test platform_config -- --test-threads=1
```

Expected: PASS; shipped configuration contains no absence assertion.

- [x] **Step 6: Commit**

```bash
git add src/bolt_v3_providers/polymarket/economics.rs tests/fixtures/economics/polymarket/explicit_zero_fee.json tests/support/economics.rs tests/bolt_v3_economics_runtime.rs config/root.toml config/profiles/prod-btc-5m.overlay.toml tests/fixtures/bolt_v3/root.toml tests/fixtures/legacy_prod_btc_5m_oracle.toml
git commit -m "fix(economics): require provider fee descriptors"
```

### Task 5: Seal maker leg, instrument, and proposal as one authority

**Files:**
- Modify: `src/bolt_v3_quote_lifecycle.rs:2750-2780`
- Modify: `src/bolt_v3_maker_quote_control.rs:35-115`
- Modify: `src/bolt_v3_maker_order_dispatch.rs:20-330`
- Modify: `src/strategies/binary_oracle_maker/mod.rs:460-610,770-810`
- Test: `src/bolt_v3_order_execution.rs`
- Test: `src/bolt_v3_order_execution/tracked_order_economics.rs`
- Test: `src/bolt_v3_maker_order_dispatch.rs` unit tests
- Test: `tests/bolt_v3_binary_oracle_maker_runtime.rs`
- Test: `tests/bolt_v3_maker_runtime_quote.rs`

**Interfaces:**
- Produces: branch-free `MakerOrderLifecycleScopeIdentity::instrument_id(leg)`, private `MakerQuoteLegAuthority`, and `MakerQuoteTransactionContext::new`.
- Removes: public fields on `MakerQuoteTransactionContext` and the redundant `MakerQuoteCommandProposal::action` field.
- Preserves: `MakerOrderCommandAuthority::ScopeCancelAll` as a separate capability.

- [x] **Step 1: Add malformed submit/cancel/modify tests**

For a market sealed as YES=`Y`, NO=`N`, construct commands with `leg=Yes` and `instrument_id=N`. Assert `LifecycleScope` failure and zero build/preparation/registration/sink counts. Cover:

```rust
MakerCompiledOrderCommand::Submit { leg: Leg::Yes, inputs: inputs_for(no_id), .. }
MakerCompiledOrderCommand::Cancel { leg: Leg::Yes, instrument_id: no_id, .. }
MakerCompiledOrderCommand::Modify { leg: Leg::Yes, instrument_id: no_id, .. }
```

Keep the existing correct YES/YES and NO/NO controls.

- [x] **Step 2: Run the mismatch tests and verify the current dispatcher accepts them**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib maker_command_rejects_leg_instrument_mismatch -- --test-threads=1
```

Expected: FAIL because `bind_to` checks only `MarketAction`.

- [x] **Step 3: Add the scope accessor and sealed context**

```rust
impl MakerOrderLifecycleScopeIdentity {
    pub(crate) const fn instrument_id(self, leg: Leg) -> InstrumentId {
        self.instrument_ids[leg as usize]
    }
}

#[derive(Debug, Clone, PartialEq)]
struct MakerQuoteLegAuthority {
    market: MarketQuote,
    budget: RequoteBudgetPair,
    proposal: MakerQuoteCommandProposal,
    instrument_id: InstrumentId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MakerQuoteTransactionContext {
    authority: MakerQuoteLegAuthority,
}
```

`new(market, budget, proposal)` derives the action from the proposal's private lifecycle and the instrument from `market.scope_identity().instrument_id(leg)`. The proposal no longer stores a second, correlated action field; the context never exposes fields.

- [x] **Step 4: Validate command and final order before mutation**

Change `bind_to` to accept the command action and command instrument. For submit, validate once before order construction and validate `order.instrument_id()` again before `prepare_maker_order`. For cancel/modify, validate before calling the sink. Return `LifecycleScope` for instrument mismatch.

- [x] **Step 5: Migrate strategy and integration-test construction**

Construct contexts only through `MakerQuoteTransactionContext::new`. `route_maker_order_command` continues to resolve exactly one active market and bind shared retention authority before dispatch.

- [x] **Step 6: Run maker authority tests**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib maker_order_dispatch -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker maker_command_rejects -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker runtime_quote_order_plan_compiles_and_dispatches_both_legs -- --test-threads=1
```

Expected: PASS with no mutation on cross-leg commands.

- [x] **Step 7: Commit**

```bash
git add src/bolt_v3_quote_lifecycle.rs src/bolt_v3_maker_quote_control.rs src/bolt_v3_maker_order_dispatch.rs src/strategies/binary_oracle_maker/mod.rs src/bolt_v3_order_execution.rs src/bolt_v3_order_execution/tracked_order_economics.rs tests/bolt_v3_binary_oracle_maker_runtime.rs tests/bolt_v3_maker_runtime_quote.rs
git commit -m "fix(maker): seal quote leg instrument authority"
```

### Task 6: Name the conservative NT mutation boundary truthfully

**Files:**
- Modify: `src/bolt_v3_quote_lifecycle.rs`
- Modify: `src/bolt_v3_maker_order_dispatch.rs`
- Modify: `src/bolt_v3_order_execution/tracked_order_economics.rs`
- Modify: `src/bolt_v3_order_execution/tracked_order_economics/cancel_coordinator.rs`
- Test: cancellation coordinator unit tests and maker transaction tests
- Test: `tests/bolt_v3_maker_runtime_quote.rs`

**Interfaces:**
- Renames: `CommandIssued` to `NtMutationInvoked` and `settle_command_issued` to `settle_nt_mutation_invoked`.
- Preserves: charging and lifecycle settlement after NT method invocation, regardless of synchronous `Result<()>`.

- [x] **Step 1: Add an NT-error accounting test**

Extend `CoordinatorSink` with a configured cancel error. Add `nt_cancel_error_after_invocation_retains_charge_and_enters_backoff`: assert one NT method call, one REST charge, retained prepaid replacement authority, `Backoff`, and quote settlement through `NtMutationInvoked`.

- [x] **Step 2: Run the test before renaming**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib nt_cancel_error_after_invocation_retains_charge_and_enters_backoff -- --test-threads=1
```

Expected: PASS as characterization evidence under the old name; the final behavior must retain the conservative charge while the boundary is renamed truthfully.

- [x] **Step 3: Rename the boundary exhaustively**

Rename the quote settlement/success/event variants, participant trait method, implementations, coordinator local `sink_invoked` flag, diagnostics, and tests. Use `nt_mutation_invoked` for the boolean and document:

```rust
// True means the NT mutation method was invoked. It does not prove that a
// network request left the process, so the reservation remains conservatively charged.
```

Do not add a cancel `SinkRejected` refund path.

- [x] **Step 4: Run cancellation and quote settlement tests**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib cancel_coordinator::tests -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib governed_transaction_state_event_table_is_total_and_replay_safe -- --test-threads=1
```

Expected: PASS and no production `CommandIssued` symbol remains.

- [x] **Step 5: Commit**

```bash
git add src/bolt_v3_quote_lifecycle.rs src/bolt_v3_maker_order_dispatch.rs src/bolt_v3_order_execution/tracked_order_economics.rs src/bolt_v3_order_execution/tracked_order_economics/cancel_coordinator.rs tests/bolt_v3_maker_runtime_quote.rs
git commit -m "refactor(maker): name NT mutation invocation boundary"
```

### Task 7: Encode requote liability phase as a variant

**Files:**
- Modify: `src/bolt_v3_requote_budget.rs:140-190,300-525`
- Test: `src/bolt_v3_requote_budget.rs` unit tests

**Interfaces:**
- Produces: phase-shaped `OutstandingLiability` variants.
- Preserves: public `RequoteBudgetReservation` API and numeric accounting.

- [ ] **Step 1: Add a two-stage cancel/resubmit accounting test**

Reserve cancel/resubmit, call `mark_sink_invoked_at` once, and assert only the replacement submit/REST liability remains outstanding. Call it a second time, commit, and assert exactly one submit plus two REST calls were charged.

- [ ] **Step 2: Run the behavior test as a baseline**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib cancel_resubmit_liability_advances_by_phase -- --test-threads=1
```

Expected: current behavior may pass; retain it as characterization evidence for the structural refactor.

- [ ] **Step 3: Replace the correlated fields**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutstandingLiability {
    OneShot {
        now_ms: u64,
        submit_cost: u64,
        rest_cost: u64,
    },
    CancelResubmitBothOutstanding {
        now_ms: u64,
        submit_cost: u64,
        cancel_rest_cost: u64,
        replacement_rest_cost: u64,
    },
    CancelResubmitReplacementOutstanding {
        now_ms: u64,
        submit_cost: u64,
        replacement_rest_cost: u64,
    },
}
```

Give the enum exhaustive `submit_cost()`, `rest_cost()`, `retimestamp()`, and settlement methods. The first cancel/resubmit sink call charges `cancel_rest_cost` and replaces the map entry with `CancelResubmitReplacementOutstanding`. Never compare a cost value to `CANCEL_RESUBMIT_REST_COST` to infer phase.

- [ ] **Step 4: Run all requote-budget tests**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib bolt_v3_requote_budget::tests -- --test-threads=1
```

Expected: PASS with unchanged public accounting.

- [ ] **Step 5: Commit**

```bash
git add src/bolt_v3_requote_budget.rs
git commit -m "refactor(maker): type requote liability phases"
```

### Task 8: Remove impossible terminal state and thread actor time through closure

**Files:**
- Modify: `src/bolt_v3_order_execution/tracked_order_economics.rs:249-290,1000-1070,1577-1635`
- Modify: `src/bolt_v3_order_execution/tracked_order_economics/cancel_coordinator.rs:930-980,1100-1145`
- Test: tracked-order and cancellation-coordinator unit tests

**Interfaces:**
- Changes: `settle_maker_terminal(..., disposition, now_ns)` and `settle_maker_terminal_authority(..., disposition, now_ns)`.
- Produces: terminal disposition first, then `MakerQuoteRetainedTerminal::Terminal(disposition)`.

- [ ] **Step 1: Add terminal-refinement and deadline tests**

Add a table covering no prior terminal, refinable terminal, reopened canceled to filled, and idempotent terminal. Add `scope_closure_uses_observed_actor_time_for_new_cancel_deadline`: close at nonzero `now_ns`, observe before the configured deadline, and assert no immediate `CancellationDeadlineExceeded`.

- [ ] **Step 2: Run the deadline regression and verify current failure**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib scope_closure_uses_observed_actor_time_for_new_cancel_deadline -- --test-threads=1
```

Expected: FAIL when the closure fallback deadline is created from zero.

- [ ] **Step 3: Compute terminal disposition before wrapping**

```rust
let authoritative = match self.retained {
    None => disposition,
    Some(MakerQuoteRetainedTerminal::Terminal(previous)) => {
        previous.refine_terminal_with(disposition)
    }
    Some(MakerQuoteRetainedTerminal::ReopenedFrom(_)) => disposition,
};
let next = MakerQuoteRetainedTerminal::Terminal(authoritative);
```

Match `(prior, authoritative)` for refinement effects, assign `next` afterward, and delete the impossible `ReopenedFrom` arm and `unreachable!`.

- [ ] **Step 4: Thread `now_ns` through every terminal path**

Pass the existing `refresh_tracked_economics` and cancel-reconciliation observation time through `settle_maker_terminal`, `settle_maker_terminal_authority`, and `settle_tracked_terminal`. Build `RetentionHorizonCapability::ScopeClosure { now_ns, .. }` from that value. Remove the `now_ns: 0` sentinel.

- [ ] **Step 5: Run terminal, horizon, and cancellation tests**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib terminal -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib cadence_scope -- --test-threads=1
```

Expected: PASS; no zero closure deadline exists.

- [ ] **Step 6: Commit**

```bash
git add src/bolt_v3_order_execution/tracked_order_economics.rs src/bolt_v3_order_execution/tracked_order_economics/cancel_coordinator.rs
git commit -m "fix(maker): bind terminal closure to actor time"
```

### Task 9: Collapse quote transaction mode and phase into one reducer

**Files:**
- Modify: `src/bolt_v3_quote_lifecycle.rs:320-780,1060-2695`
- Test: `src/bolt_v3_quote_lifecycle.rs:3820-4160`
- Test: `tests/bolt_v3_binary_oracle_maker_runtime.rs`
- Test: `tests/bolt_v3_maker_runtime_quote.rs`

**Interfaces:**
- Produces: `QuoteStableState`, `QuoteAttemptState`, `QuoteAttemptPhase`, and `QuoteSettlementState`.
- Removes: `WindDownQuoteTransactionState`, recursive boxed settlement, `route: Option<_>`, and `reopened: bool`.
- Preserves: all `MarketQuote` and participant public/crate-visible methods.

- [ ] **Step 1: Add active/wind-down equivalence characterization**

Create `active_and_winding_down_attempts_share_accounting_and_terminal_semantics`. Drive equivalent armed and sink-invoked cancel attempts, wind one down, settle both, and assert identical reservation retirement plus mode-specific final leg state. Keep the existing total event table and terminal matrix.

- [ ] **Step 2: Run the quote lifecycle baseline**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib bolt_v3_quote_lifecycle::tests -- --test-threads=1
```

Expected: existing tests pass before refactor; the new equivalence test characterizes behavior.

- [ ] **Step 3: Introduce the non-recursive state product**

```rust
enum QuoteTransactionState {
    Stable(QuoteStableState),
    Attempt(QuoteAttemptState),
    Settled(QuoteSettlementState),
}

enum QuoteStableState {
    Active(ActiveQuotePhase),
    WindingDown(WindDownQuotePhase),
    Poisoned(QuotePoisonedHold),
}

struct QuoteAttemptState {
    mode: QuoteTransactionMode,
    arm: QuoteTransactionArm,
    phase: QuoteAttemptPhase,
}

enum QuoteAttemptPhase {
    Armed(ArmedQuoteBudget),
    SinkInvoked(SinkInvokedQuoteBudget),
}

enum QuoteSettlementState {
    AwaitingRoute {
        generation: u64,
        stable: QuoteStableState,
    },
    RouteSettled {
        generation: u64,
        route: QuoteRouteSettlement,
        stable: QuoteStableState,
    },
    Reopened {
        generation: u64,
        route: QuoteRouteSettlement,
        stable: QuoteStableState,
    },
}
```

`QuotePoisonedHold` carries mode, obligation, and typed budget ownership. `ActiveQuotePhase` owns active-only resting/replacement states; `WindDownQuotePhase` owns only idle/cancel-pending stable states.

- [ ] **Step 4: Migrate arm/sink/unwind through one attempt reducer**

Delete `QuoteTransactionMode::{armed_state,sink_invoked_state}`, `ClassifiedSinkInvokedState`, and all mirrored wind-down arms. Each attempt operation matches `QuoteAttemptPhase` once. Mode is read only when choosing the resulting `QuoteStableState`.

- [ ] **Step 5: Make wind-down one-way and settlement explicit**

Wind-down converts an active stable state once, flips `QuoteAttemptState.mode`, or flips the poison hold mode. Settlement replay exhaustively distinguishes `AwaitingRoute`, matching `RouteSettled`, conflicting/stale settlement, and `Reopened`. A settlement owns `QuoteStableState`, never `QuoteTransactionState`.

- [ ] **Step 6: Update introspection without recreating Cartesian matches**

Implement `leg_state`, `prepaid_generation`, `is_winding_down`, `armed_identity`, `generation`, and `registration_phase` on the smallest owning type, then delegate top-down. Do not reproduce mode-by-phase pairs in each accessor.

- [ ] **Step 7: Run quote, maker, and cancellation behavior suites**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib bolt_v3_quote_lifecycle::tests -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib cancel_coordinator::tests -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker -- --test-threads=1
```

Expected: PASS; `WindDownQuoteTransactionState` has zero references.

- [ ] **Step 8: Inspect complexity before committing**

Run exact symbol/branch checks:

```bash
rg -n 'WindDownQuoteTransactionState|route: Option<QuoteRouteSettlement>|reopened: bool' src/bolt_v3_quote_lifecycle.rs
rg -n '^(\s*)(if\b|match\b)' src/bolt_v3_quote_lifecycle.rs
```

Expected: first command has no matches. Review the second output to ensure helpers delegate instead of rebuilding parallel reducers.

- [ ] **Step 9: Commit**

```bash
git add src/bolt_v3_quote_lifecycle.rs tests/bolt_v3_binary_oracle_maker_runtime.rs tests/bolt_v3_maker_runtime_quote.rs
git commit -m "refactor(maker): collapse quote transaction phases"
```

### Task 10: Delete dormant current forced-reduction submit authority

**Files:**
- Modify: `src/bolt_v3_order_execution/economics_basis.rs`
- Modify: `src/bolt_v3_order_execution.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/bolt_v3_current_evidence/{facts.rs,reader.rs,record.rs,handles.rs}`
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `src/bolt_v3_kill_switch_{flatten,action_router}.rs`
- Modify tests: `tests/{bolt_v3_submit_admission.rs,bolt_v3_capital_admission_runtime_feed.rs,bolt_v3_current_evidence_runtime.rs,bolt_v3_kill_switch_flatten.rs,bolt_v3_kill_switch_action_router.rs}`
- Modify fixtures/support: `src/bolt_v3_live_node/tests/fixtures.rs`, `tests/support/current_evidence.rs`

**Interfaces:**
- Removes: `BoltV3FinalOrderEconomicsScenario::ForcedReduction`, `BoltV3SubmitIntentKind::KillSwitchForcedReduction`, forced-reduction request field/policy/evaluator/liveness sets/current recorder.
- Preserves: historical `ForcedReductionAdmissionFact` decoding and proof-only flatten planning.
- Changes: historical forced-reduction facts are ignored by `ReservationRecoveryFacts` and cannot satisfy `authorizes_order`.

- [ ] **Step 1: Add the historical-evidence inertness regression**

Read the existing forced-reduction JSONL fixture through the current reader. Assert it still decodes as `CurrentFact::ForcedReductionAdmission`, then build reservation recovery facts and assert:

```rust
assert!(!recovery.authorizes_order("the-historical-client-order-id"));
assert!(!recovery.authorizes_non_reservation_order("the-historical-client-order-id"));
```

- [ ] **Step 2: Run the regression and verify current failure**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test bolt_v3_current_evidence_runtime historical_forced_reduction_fact_is_decode_only -- --test-threads=1
```

Expected: FAIL because admitted historical forced reduction currently populates recovered submit authority.

- [ ] **Step 3: Delete economics and submit intent variants**

Remove the scenario variant/constructor and every related branch from `economics_basis.rs` and `order_execution.rs`. Reduce the intent enum to:

```rust
pub enum BoltV3SubmitIntentKind {
    Entry,
    RiskReducingExit,
}
```

Delete the impossible normal-admission arm and update all exhaustive matches.

- [ ] **Step 4: Delete admission authority and liveness state**

Remove the request claim field, admission policy/configuration, evaluator, live forced-reduction ID set, forced-reduction reconciliation flag, special counters/rollback, current evidence authority, and current error mapping. `CapitalAdmissionRebuildSnapshot` retains only `live_non_reservation_client_order_ids`.

- [ ] **Step 5: Make historical recovery inert and remove current producers**

Keep `ForcedReductionAdmissionFact`, its identity, decoder, and fixture. Remove `ReservationRecoveryEvent::ForcedReduction`, the recovered set/accessor, and the reader mapping into reservation authority; the reader must decode then skip that fact for authorization. Remove `record_forced_reduction_admission` from production recorder/handle APIs and remove runtime fixtures that call it.

- [ ] **Step 6: Retain proof-only kill-switch planning outside submit admission**

Move `BoltV3KillSwitchForcedReductionClaim` and its validation error to `bolt_v3_kill_switch_flatten.rs`; import it from `bolt_v3_kill_switch_action_router.rs`. Delete `BoltV3KillSwitchForcedReductionPolicy`, which exists only for submit admission. Keep the existing loaded-config rejection of automatic flattening and proof-only planner tests.

- [ ] **Step 7: Delete obsolete tests and migrate unaffected fixtures**

Delete tests whose sole purpose is admitting/rate-limiting a current forced-reduction submit. Preserve codec decode/round-trip tests, flatten-plan proof tests, and config rejection tests. Remove forced intent branches from shared test helpers rather than replacing them with dummy behavior.

- [ ] **Step 8: Run targeted admission, evidence, and kill-switch tests**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib economics_basis::tests -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test admission_orders -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test bolt_v3_current_evidence_runtime -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test kill_switch_loss -- --test-threads=1
```

Expected: PASS; historical evidence decodes but grants no current authority.

- [ ] **Step 9: Confirm current authority symbols are gone**

```bash
rg -n 'BoltV3FinalOrderEconomicsScenario::ForcedReduction|BoltV3SubmitIntentKind::KillSwitchForcedReduction|record_forced_reduction_admission|authorizes_forced_reduction_order|live_kill_switch_forced_reduction_client_order_ids' src tests
```

Expected: no matches. Historical fact/codec names may remain and must be inspected separately.

- [ ] **Step 10: Commit**

```bash
git add src tests
git commit -m "refactor(admission): delete dormant forced reduction authority"
```

### Task 11: Correct lasting scope evidence and verify the exact head

**Files:**
- Modify: PR #1544 body (stable scope only; external state)
- Modify: issue #869 with a durable tracking comment (external state)
- Add: exact-head PR review-request comment (external state)

**Interfaces:**
- Produces: corrected conditional census, explicit #869 remainder, exact-head verification record, and fresh review requests.
- Does not produce: merge authority or a merge.

- [ ] **Step 1: Run formatting and cheap static checks**

```bash
cargo fmt --all -- --check
git diff --check e62584045629208e81d2dce1fce608720ea01fbf...HEAD
rg -n 'TODO|FIXME' src tests config
rg -n 'WindDownQuoteTransactionState|absent_fee_descriptor_policy|KillSwitchForcedReduction|settle_command_issued|CommandIssued' src config tests
```

Expected: formatting/diff clean; no newly introduced debt; removed current-authority symbols absent. Historical codec names are adjudicated, not blindly deleted.

- [ ] **Step 2: Run the affected local behavior suites sequentially**

```bash
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib binary_oracle_edge_taker::tests::exposure -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib bolt_v3_quote_lifecycle::tests -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --lib cancel_coordinator::tests -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test maker_taker -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test wiring_registration -- --test-threads=1
CARGO_TARGET_DIR='/Volumes/T9/bolt-v2-target-1544-review-repairs' CARGO_BUILD_JOBS=2 cargo test --locked --features test-current-evidence-inspection --test admission_orders -- --test-threads=1
```

Expected: all pass. If a target is too broad for the local economic budget, stop after the smallest failing named test and repair it; do not run Cargo concurrently.

- [ ] **Step 3: Compute and inspect the conditional census**

Use the same production-Rust filter for both ranges: include `src/**` and `crates/*/src/**`; exclude `tests`, `benches`, path components named `tests`, `#[cfg(test)]` items, comments, strings, and character literals. Report added/removed/net lines containing lexical `if` and lexical `match` separately, plus per-file totals, for:

```text
623801311..HEAD
524362b68..HEAD
```

Record the exact diagnostic command/script in the review comment, not in the repository. Required gate: affected production files over `524362b68..HEAD` have a negative combined `if`+`match` net. If not, revisit Task 2 or Task 9 before review.

- [ ] **Step 4: Perform an internal adversarial diff review**

Inspect `git diff --stat`, `git diff --check`, every changed production file, all deleted authorities, and test evidence. Build a finding-to-repair table for all 14 adjudicated findings. Repair every substantive local finding before continuing.

- [ ] **Step 5: Commit any final test-only or documentation corrections**

```bash
git add <only-the-reviewed-final-files>
git commit -m "test(economics): close external review regressions"
```

Skip this commit when the worktree is already clean; never create an empty commit.

- [ ] **Step 6: Update lasting GitHub scope records**

Add a durable #869 comment naming the deferred maker NT-event-to-lifecycle/event-fence work inherited from closed #817. Update PR #1544's stable body to name the takeover-round exposure authority, maker transaction boundary, load-time OMS capability work, this structural repair round, and #869 remainder. Do not put head SHA, transient check state, or per-head receipts in the PR body.

- [ ] **Step 7: Push exact head and detach from CI**

```bash
git status --short
git rev-parse HEAD
git push
```

Expected: clean worktree and successful plain push. Do not wait for advisory CI.

- [ ] **Step 8: Post exact-head review evidence and request reviews**

Post a PR comment containing head/base SHA, finding-to-repair table, targeted local commands/results, corrected census with script, and explicit remote checks requested: root workspace fmt/Clippy/nextest/build plus isolated backtesting fmt/Clippy. Request fresh Claude and GPT reviews of the exact head, then request/re-request native review from the login resolved from node `U_kgDOEZMFhA` and verify that it is `sp-reviewer` at request time.

- [ ] **Step 9: Report without merging**

Report the pushed head SHA, PR URL, review-request links, local evidence, and pending advisory/native/external reviews. Do not merge or wait on CI.
