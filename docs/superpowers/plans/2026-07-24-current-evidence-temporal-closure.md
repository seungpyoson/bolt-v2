# Current Evidence Temporal Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete issue #1354's current-only decision-evidence cutover by making every admitted durable prefix, writer lifetime, recovery projection, and operator-health transition closed and compiler-enforced.

**Architecture:** One catalog-scoped Unix runtime owns the parsed evidence namespace, a nonblocking exclusive catalog lock, two durable sinks, and an explicit `Open -> Closing -> Closed` lifecycle. Admission authorization and reservation attribution commit in one fact; settlement recovery reconstructs one mutually exclusive terminal outcome and idempotently reapplies every authorized runtime effect. Component-scoped weak handles are the only append capabilities, and all live/BTE readers share one positive finite cap type and the same runtime implementation.

**Tech Stack:** Rust 2024, `libc` Unix descriptor APIs, Serde JSONL codecs, TOML-generated contract, NautilusTrader Rust APIs, cargo-nextest, GitHub advisory workflows.

## Global Constraints

- Governed by repository `AGENTS.md`.
- Owning slice is issue #1354 only.
- Issue #1385 retains rotation, total retained-capacity enforcement, retirement, durable ordinals, and append-retry exact-once across restart.
- No compatibility decoder, migration path, alternate writer/reader, persisted lock, fallback runtime mode, source-scanning test, or duplicated health state.
- Pre-cutover evidence remains archived and unavailable to the current runtime.
- Live cutover remains unauthorized.
- Machine corruption fails construction; observation corruption preserves bytes and poisons only observation recording.
- A receipt/token exists only after `write_all` and `sync_data` succeed.
- Tests verify behavior and types, not source text.
- Production clippy stays scoped to production targets; full no-fail-fast tests compile and exercise test targets.

---

### Task 1: Restore the trustworthy verification baseline

**Files:**
- Modify: `.github/workflows/advisory.yml`
- Modify: `justfile`
- Modify: `src/bolt_v3_live_node/tests/startup_rebuild.rs`
- Modify: `tests/bolt_v3_current_evidence_runtime.rs`
- Modify: `tests/config_parsing.rs`

**Interfaces:**
- Consumes: existing advisory root/BTE commands.
- Produces: one canonical production-target clippy command and full no-fail-fast test commands.

- [ ] **Step 1: Replace brittle failure assertions with owned-boundary assertions**

Use the typed `ObservationStreamStatus::Poisoned`/`PoisonCause` value in `startup_rebuild.rs`; derive the Shadow-PnL fixture set by `sink_for_identity(identity) == KnownSink::Machine`; assert the stable `persistence.decision_evidence` field identity rather than the full config error sentence.

- [ ] **Step 2: Run the three exact failing tests and verify they fail before edits**

Run:

```bash
cargo nextest run --locked --features test-current-evidence-inspection \
  bolt_v3_live_node::tests::startup_rebuild::live_node_surfaces_poisoned_observation_stream_without_gating_startup \
  shadow_pnl_dispositions_have_typed_reducers_for_the_complete_current_corpus \
  config_parsing::rejects_colliding_decision_evidence_paths
```

Expected: the three known deterministic failures from advisory run `30017159505`.

- [ ] **Step 3: Implement the assertion and fixture-boundary corrections**

The Shadow-PnL case must select fixtures from the generated sink mapping, not a hand-maintained identity list.

- [ ] **Step 4: Restore production-target clippy without lint suppression**

Use:

```yaml
cargo clippy --locked --features test-current-evidence-inspection --lib --bins -- -D warnings
```

Keep root `nextest` on `--no-fail-fast --features test-current-evidence-inspection`. Keep the BTE MinIO exclusion limited to its named opt-in module.

- [ ] **Step 5: Verify the corrected baseline**

Run the three targeted tests, `cargo fmt --check`, and the production-target clippy command. Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/advisory.yml justfile \
  src/bolt_v3_live_node/tests/startup_rebuild.rs \
  tests/bolt_v3_current_evidence_runtime.rs tests/config_parsing.rs
git commit -m "test: restore current evidence verification baseline"
```

---

### Task 2: Close path identity, read capacity, and individual-frame bounds

**Files:**
- Modify: `src/bolt_v3_current_evidence/path.rs`
- Modify: `src/bolt_v3_current_evidence/path_authority.rs`
- Modify: `src/bolt_v3_current_evidence/reader.rs`
- Modify: `src/bolt_v3_current_evidence/runtime.rs`
- Modify: `src/bolt_v3_current_evidence/record.rs`
- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_validate/persistence.rs`
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`
- Test: `tests/bolt_v3_current_evidence_runtime.rs`
- Test: `tests/config_parsing.rs`

**Interfaces:**
- Produces:

```rust
pub struct CanonicalRelativeEvidencePath(Box<[OsString]>);

impl CanonicalRelativeEvidencePath {
    pub fn parse(raw: &str) -> Result<Self>;
    pub fn is_ancestor_of(&self, other: &Self) -> bool;
    pub fn components(&self) -> impl Iterator<Item = &OsStr>;
}

#[derive(Clone, Copy)]
pub struct PositiveFiniteEvidenceReadCap(NonZeroU64);

impl PositiveFiniteEvidenceReadCap {
    pub fn new(value: u64) -> Result<Self>;
    pub fn value(self) -> u64;
    pub fn sentinel(self) -> u64;
}
```

- Consumes: live TOML and BTE manifest cap/path values.

- [ ] **Step 1: Write failing path topology tests**

Cover redundant separators, `.` components, trailing separators, active/active equality and ancestry in both directions, active/retired equality and ancestry in both directions, and assert the catalog is unchanged after rejection.

- [ ] **Step 2: Write failing cap and frame-bound tests**

Cover `0`, `u64::MAX`, exact-cap success, cap+1 read rejection, growth-during-read rejection, and an atomic record whose encoded line is cap+1 bytes being rejected before I/O.

- [ ] **Step 3: Run the focused tests and verify expected failures**

Run:

```bash
cargo nextest run --locked --features test-current-evidence-inspection \
  -E 'test(/decision_evidence_path/)|test(/evidence_read_cap/)|test(/frame_bound/)'
```

Expected: failures showing raw-string aliases, accepted `u64::MAX`, or missing write-side frame bound.

- [ ] **Step 4: Implement the parsed path and finite-cap types**

Reject noncanonical spelling rather than normalizing it silently. Validate the complete active/retired topology before opening or creating anything. Pass parsed components directly into every descriptor-relative walk.

- [ ] **Step 5: Bound consumed reads and encoded records**

Replace `saturating_add(1)` with `PositiveFiniteEvidenceReadCap::sentinel()` using checked construction. Make every public semantic reader and startup validator accept the cap type. Before `write_all`, reject `encoded.len() > cap.value()` with a typed non-I/O error.

- [ ] **Step 6: Thread the same cap through live config, BTE manifests, and offline runtime**

Delete the offline `0` exception and raw `u64` reader signatures.

- [ ] **Step 7: Run focused tests and formatting**

Expected: all new path/cap/frame tests PASS; existing symlink, hard-link, parent-sync, and byte-cap tests remain green.

- [ ] **Step 8: Commit**

```bash
git add src/bolt_v3_current_evidence src/bolt_v3_config.rs \
  src/bolt_v3_validate/persistence.rs \
  crates/backtesting-vertical-slice/src/runner.rs \
  tests/bolt_v3_current_evidence_runtime.rs tests/config_parsing.rs
git commit -m "feat: close evidence path and capacity authority"
```

---

### Task 3: Establish catalog-scoped single-writer ownership

**Files:**
- Modify: `src/bolt_v3_current_evidence/path_authority.rs`
- Modify: `src/bolt_v3_current_evidence/runtime.rs`
- Modify: `src/bolt_v3_current_evidence/mod.rs`
- Modify: `src/bolt_v3_operator_artifacts.rs`
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`
- Test: `tests/bolt_v3_current_evidence_runtime.rs`

**Interfaces:**
- Produces:

```rust
struct LockedEvidenceCatalog {
    directory: CatalogDirectory,
}

impl LockedEvidenceCatalog {
    fn open_and_lock(path: &Path) -> Result<Self, RuntimeOpenError>;
}

enum RuntimeOpenError {
    WriterAlreadyActive,
    // existing typed failures
}
```

- [ ] **Step 1: Write real two-process ownership tests**

Cover same catalog/same streams, same catalog/different configured streams, concurrent first open, restart overlap, child crash release, conflict byte preservation, and independent BTE temporary catalogs.

- [ ] **Step 2: Verify the tests fail because both processes can construct append capability**

Run the ownership test target on Unix. Expected: second process opens successfully at the current head.

- [ ] **Step 3: Acquire `LOCK_EX | LOCK_NB` on the held catalog directory descriptor**

Ordering is parse/validate topology -> open catalog descriptor -> lock descriptor -> retired checks -> parent/stream mutation -> validation -> capability. A lock conflict must perform no evidence namespace mutation. Retain the exact descriptor until runtime close; never explicitly unlock.

- [ ] **Step 4: Delete raw-file offline construction**

Replace `OfflineDecisionEvidenceRuntime::from_fresh_files(File, File, ...)` with an isolated temporary catalog that uses the same `LockedEvidenceCatalog` and stream opening path as live.

- [ ] **Step 5: Route launch-identity artifacts through the same held catalog authority**

Remove pathname `create_dir_all` inside the evidence catalog namespace. Use descriptor-relative, no-follow, create-new semantics.

- [ ] **Step 6: Verify Linux/macOS-compatible behavior**

Run focused process tests locally where supported; leave remote advisory evidence to prove both repository target platforms. Non-Unix construction remains an explicit error.

- [ ] **Step 7: Commit**

```bash
git add src/bolt_v3_current_evidence src/bolt_v3_operator_artifacts.rs \
  crates/backtesting-vertical-slice/src/runner.rs \
  tests/bolt_v3_current_evidence_runtime.rs
git commit -m "feat: enforce single current evidence writer"
```

---

### Task 4: Close all internally produced semantic domains

**Files:**
- Modify: `src/bolt_v3_loss_governor.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/exit_decision.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/bolt_v3_current_evidence/facts.rs`
- Modify: `src/bolt_v3_current_evidence/codec/*.rs`
- Test: codec corpus and strategy behavior tests

**Interfaces:**
- Produces closed producer enums for exit blocked reason, loss snapshot source, lifecycle source, requote leg, selection phase, order type, time-in-force, settlement entry side, and repeated RV/forced-flat diagnostics.
- Keeps genuinely external venue reason text and identifiers as text.

- [ ] **Step 1: Add failing exhaustive-domain tests**

Use compiler-exhaustive matches and wire rejection mutations; do not add source scans.

- [ ] **Step 2: Verify the existing string conversion/panic/fallback cases fail the desired tests**

- [ ] **Step 3: Replace producer-side strings with closed enums**

Delete `exit_block_reason_to_evidence(&str)`, wildcard `unreachable!`, loss-source `_ => Other`, duplicated internal literals, and `format!("{:?}")` wire projections. Derive labels only in total typed-to-text presentation functions.

- [ ] **Step 4: Regenerate/freeze identity-local wire enums and fixtures**

Unknown values must reject at the owned codec boundary. Open external text remains explicitly documented and validated as text.

- [ ] **Step 5: Run codec corpus, strategy, and clippy checks**

- [ ] **Step 6: Commit**

```bash
git add src/bolt_v3_loss_governor.rs src/bolt_v3_submit_admission.rs \
  src/strategies/binary_oracle_edge_taker \
  src/bolt_v3_current_evidence tests
git commit -m "refactor: close decision evidence producer domains"
```

---

### Task 5: Replace standalone reservation metadata with atomic admitted facts

**Files:**
- Modify: `config/decision-evidence-contract.toml`
- Modify: `src/bolt_v3_current_evidence/contract_generator.rs`
- Regenerate: `src/bolt_v3_current_evidence/generated_contract.rs`
- Modify: `src/bolt_v3_current_evidence/facts.rs`
- Modify: `src/bolt_v3_current_evidence/codec/admission.rs`
- Delete metadata codec path from: `src/bolt_v3_current_evidence/codec/reservation.rs`
- Modify: `src/bolt_v3_current_evidence/record.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_basket_admission.rs`
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `crates/backtesting-vertical-slice/src/runner.rs`
- Modify: `tests/fixtures/current_evidence/**`
- Test: admission, basket, recovery, codec, and BTE tests

**Interfaces:**
- Produces:

```rust
pub struct ReservationAttribution {
    pub client_order_id: String,
    pub instrument_id: String,
    pub submit_reservation_id: String,
    // existing recovery-required attribution fields
}

pub struct AdmittedEntryAdmissionFact {
    // existing admitted fields
    pub reservation: Option<ReservationAttribution>,
}

pub struct BasketAdmittedLeg {
    pub client_order_id: String,
    pub instrument_id: String,
    pub reservation: Option<ReservationAttribution>,
}

pub struct BasketAdmissionGrantedFact {
    pub details: BasketAdmissionDetails,
    pub legs: Vec<BasketAdmittedLeg>,
}

#[must_use]
pub struct CommittedAdmission { /* private */ }
```

- Current risk-reducing and forced-reduction identities have no reservation field. A future capital-backed risk-reduction identity would require separate registration and policy; it is not added here.

- [ ] **Step 1: Write failing durable-prefix tests**

Cover single admitted/no NT order, admitted/matching NT order, NT order without attribution, atomic basket with 0..N submitted legs, duplicate client IDs, duplicate reservation IDs, instrument/order mismatch, wrong leg ordering, rejected fact carrying attribution (unconstructible), and cap+1 basket rejection before I/O.

- [ ] **Step 2: Verify current multi-append behavior exposes orphan metadata and partial basket prefixes**

- [ ] **Step 3: Introduce nested attribution and atomic admission schemas**

Delete `SubmitReservationMetadataFact`, its producer/purpose/identity/codec/fixture/recorder method/dispositions. Embed attribution only in admitted entry and basket-grant facts.

- [ ] **Step 4: Make the outer basket component the sole basket fact producer**

The submit-admission subroutine prepares reservations and rollback state; it does not append basket grant/rejection facts.

- [ ] **Step 5: Return and consume `CommittedAdmission`**

The only permit constructor consumes the must-use token by value. Any encode/write/sync failure rolls back and returns no permit; no submit call can occur.

- [ ] **Step 6: Retarget reservation recovery**

Project attribution from admitted facts. Retain standalone fill facts and require every fill to match the embedded client order, reservation ID, instrument, and side. Duplicate/conflicting relations fail startup.

- [ ] **Step 7: Retarget BTE typed run-guard accounting**

Count embedded entry reservations plus basket leg reservations through registered typed events.

- [ ] **Step 8: Regenerate and verify the contract/corpus**

Expected relation after deletion: 25 producers, 24 purposes, 24 identities/facts, 5 consumers, 120 dispositions, subject to generator output.

- [ ] **Step 9: Commit**

```bash
git add config/decision-evidence-contract.toml src/bolt_v3_current_evidence \
  src/bolt_v3_submit_admission.rs src/bolt_v3_basket_admission.rs \
  src/bolt_v3_live_node.rs crates/backtesting-vertical-slice \
  tests/fixtures tests
git commit -m "feat: make admission evidence atomic"
```

---

### Task 6: Make settlement recovery a complete terminal-state authority

**Files:**
- Modify: `src/bolt_v3_current_evidence/facts.rs`
- Modify: `src/bolt_v3_current_evidence/codec/settlement.rs`
- Modify: `src/bolt_v3_settlement_booking.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/bolt_v3_loss_protection.rs`
- Test: settlement, recovery, venue-truth, loss-governor, and codec tests

**Interfaces:**
- Produces:

```rust
pub enum RecoveredSettlementOutcome {
    Successful(SettlementFact),
    BookingTerminal(TerminalSettlementFact),
}

#[must_use]
pub struct RecoveredSettlement { /* validated key + outcome */ }
```

- [ ] **Step 1: Write failing prefix-equivalence tests**

Cover crash after settlement sync before either reducer, between reducers, after reducers before `Flat`, booking terminal before health publication, duplicate success, duplicate terminal, success+terminal conflict, terminal without booking error, wrong lifecycle transition/outcome, and no-reappend assertions.

- [ ] **Step 2: Verify the loss replay, Flat reconstruction, and contradiction tests fail at the current head**

- [ ] **Step 3: Replace settlement key sets with one mutually exclusive outcome map**

Reject duplicate/conflicting keys. Require booking-terminal evidence to contain a booking error and exact `SettlementBookingTerminal`/`Flat` lifecycle semantics.

- [ ] **Step 4: Implement idempotent recovery application**

For recovered success: construct `RecoveredSettlement`, replay loss PnL using the persisted event-key dedupe, replay venue truth, set exposure `Flat`, release subscriptions/state, and never append. For recovered booking terminal: set `Flat`, publish degraded settlement health, and never append.

- [ ] **Step 5: Delete impossible canonical-reappend recovery branches**

Recovered terminal evidence is already durable by construction.

- [ ] **Step 6: Run the prefix-equivalence and full settlement test set**

Expected: restart state equals uninterrupted state for every accepted prefix.

- [ ] **Step 7: Commit**

```bash
git add src/bolt_v3_current_evidence src/bolt_v3_settlement_booking.rs \
  src/strategies/binary_oracle_edge_taker/mod.rs \
  src/bolt_v3_loss_protection.rs tests
git commit -m "fix: make settlement recovery equivalent"
```

---

### Task 7: Seal recorder capability and lifecycle

**Files:**
- Modify: `src/bolt_v3_current_evidence/record.rs`
- Modify: `src/bolt_v3_current_evidence/runtime.rs`
- Modify: `src/bolt_v3_current_evidence/mod.rs`
- Modify: `src/bolt_v3_strategy_context.rs`
- Modify: component wiring in `src/bolt_v3_live_node.rs`
- Modify: order execution, submit admission, basket admission, maker, edge taker, and reject-observer callers
- Modify: BTE wiring
- Test: compile/type behavior and shutdown concurrency tests

**Interfaces:**
- Produces:

```rust
enum RecorderLifecycle {
    Open { active: usize },
    Closing { active: usize },
    Closed,
}

struct RecorderCore { /* only strong runtime-owned core */ }
struct ActiveEvidenceLease { /* drops active count */ }

pub(crate) struct OrderExecutionEvidenceHandle(Weak<RecorderCore>);
pub(crate) struct SubmitAdmissionEvidenceHandle(Weak<RecorderCore>);
pub(crate) struct BasketAdmissionEvidenceHandle(Weak<RecorderCore>);
pub(crate) struct EdgeTakerEvidenceHandle(Weak<RecorderCore>);
pub(crate) struct MakerEvidenceHandle(Weak<RecorderCore>);
pub(crate) struct OrderRejectObserverEvidenceHandle(Weak<RecorderCore>);

pub enum MustPrecedeCommit<T> { Committed(T) }
pub enum ReconciliationCommit<T> { Committed(T) }
pub enum PreserveResultEvidence { Recorded, Failed(RecordFailure) }
pub enum RiskReducingEvidence { Recorded, Failed(RecordFailure) }
pub enum ObservationEvidence { Recorded, Failed(ObservationFailure) }
```

- [ ] **Step 1: Write compile-fail/API and lifecycle tests**

Prove unrelated components cannot call settlement/admission/recovery methods; new leases fail with `RecorderClosing`/`RecorderClosed`; in-flight appends finish before descriptors/lock close; retained weak handles cannot prolong ownership.

- [ ] **Step 2: Verify the current full recorder distribution and Arc lifetime violate the tests**

- [ ] **Step 3: Make `RecorderCore` private and runtime-owned**

Remove the full recorder from `StrategyBuildContext`. Issue component handles once at composition. Handles may be cloned only inside their owning component wiring and contain `Weak<RecorderCore>`.

- [ ] **Step 4: Bind exact effect policy at handle methods**

Generated policy witnesses bind every purpose to its exact result family. Do not collapse `MustPrecedeNewRisk` and `ReconciliationFailClosed`, or `PreserveResult` and `RiskReducingContinues`, at caller-visible boundaries.

- [ ] **Step 5: Implement explicit lifecycle and active leases**

Shutdown order: stop ingress -> stop/join all evidence producers while Open -> transition Closing -> reject new leases/no I/O -> await active append/publication leases -> close observation/machine -> close catalog descriptor/release lock -> Closed.

- [ ] **Step 6: Consolidate purpose-keyed fault injection**

Keep one feature-gated purpose/attempt injector in the core. Delete per-purpose wrapper growth. Drive generated purpose x policy proof plus one behavior test per distinct caller-control class.

- [ ] **Step 7: Run focused type/lifecycle tests and clippy**

- [ ] **Step 8: Commit**

```bash
git add src crates/backtesting-vertical-slice tests
git commit -m "refactor: seal evidence capability and lifecycle"
```

---

### Task 8: Publish exactly one poison transition outside sink locks

**Files:**
- Modify: `src/bolt_v3_current_evidence/record.rs`
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `src/bolt_v3_operator_health.rs`
- Test: live-node health and recorder concurrency tests

**Interfaces:**
- Produces:

```rust
struct FirstSinkPoison {
    sink: KnownSink,
    cause: PoisonCause,
}

struct EvidenceHealthPublisher {
    // status-only weak publisher; cannot append
}
```

- [ ] **Step 1: Write failing transition/concurrency tests**

Cover startup observation poison, mid-run observation write and sync poison, machine poison, simultaneous first failures, no callback under sink lock, no recorder reentry, exactly one transition, later no-I/O attempts, episode suppression, and shutdown racing publication.

- [ ] **Step 2: Verify current code has authoritative query state but no immediate transition**

- [ ] **Step 3: Return the first-edge token internally**

Create it under the sink mutex only on `Healthy -> Poisoned`; release the mutex; the central recording façade consumes it synchronously through the status-only weak publisher. Individual producers never receive the token.

- [ ] **Step 4: Integrate publication with the active-operation lease**

The lease remains active until publication completes; Closing waits. Startup poison publishes once after composition. Sink state remains the sole authority.

- [ ] **Step 5: Add both machine and observation status to operator health**

Machine poison degrades operator health and continues to gate machine writes/readiness according to existing fail-closed policy; observation poison remains non-gating.

- [ ] **Step 6: Run focused health and deadlock tests**

- [ ] **Step 7: Commit**

```bash
git add src/bolt_v3_current_evidence/record.rs \
  src/bolt_v3_live_node.rs src/bolt_v3_operator_health.rs tests
git commit -m "feat: publish evidence poison transitions"
```

---

### Task 9: Close NT reconciliation ordering and shutdown producer ownership

**Files:**
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `src/bolt_v3_capital_admission_runtime_feed.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Test: live-node startup/reconciliation and shutdown tests

**Interfaces:**
- Consumes: NT startup ordering in which reconciliation completes before trader/strategies start.
- Produces: a Bolt admission gate that cannot become actionable until the reconciled order set is attributable.

- [ ] **Step 1: Write failing startup ordering tests**

Inject a reconciliation-discovered open order absent from the pre-run cache. Assert it reaches the capital feed as unattributed and the admission gate is unreconciled before any strategy submit path can run. Also cover no open orders, fully attributed recovered orders, filtered/unclaimed order policy, and reconciliation failure/timeout.

- [ ] **Step 2: Verify the present pre-run rebuild alone cannot prove the invariant**

- [ ] **Step 3: Make the gate depend on reconciled runtime-feed authority**

The pre-run cache snapshot is advisory seed only. Initial order-lifecycle attribution is unreconciled. NT reconciliation events update the feed before trader start; only an authoritative reconciled snapshot with every live order attributed can open capital admission. Do not add a Bolt acknowledgement journal or duplicate NT reconciliation.

- [ ] **Step 4: Put every recorder-writing producer in the shutdown bundle**

Include `loss_runtime_feed_subscription` and any handle-owning background task. Stop/join them before `RecorderCore::begin_close`.

- [ ] **Step 5: Run startup ordering and shutdown tests**

- [ ] **Step 6: Commit**

```bash
git add src/bolt_v3_live_node.rs \
  src/bolt_v3_capital_admission_runtime_feed.rs \
  src/bolt_v3_submit_admission.rs tests
git commit -m "fix: close reconciliation and shutdown ordering"
```

---

### Task 10: Reconcile generated artifacts, documentation, and final evidence

**Files:**
- Modify: generated contract/corpus files
- Modify: `docs/bolt-v3/2026-04-25-bolt-v3-schema.md`
- Modify: `docs/bolt-v3/2026-04-25-bolt-v3-runtime-contracts.md`
- Modify: `docs/runbooks/current-decision-evidence-hard-cutover.md`
- Modify: `docs/bolt-v3/shadow-mode-pnl.md`
- Modify: `crates/backtesting-vertical-slice/README.md`
- Modify: PR body only if lasting claims changed; never add transient head/check status

**Interfaces:**
- Consumes: all completed tasks.
- Produces: one mutually consistent implementation, generated contract, corpus, active docs, and review request.

- [ ] **Step 1: Regenerate the contract and committed wire corpus**

Byte-compare generated output. Ensure every identity, enum branch, optional state, semantic rejection, sink membership, and typed consumer agreement is mechanically covered.

- [ ] **Step 2: Update active documentation**

Document atomic admission, catalog ownership, frame/read caps, settlement terminal outcomes/replay, lifecycle/shutdown, immediate poison publication, NT reconciliation ordering, accepted hard-cutover losses, and #1385 boundaries. Remove contradictory predecessor authority rather than layering another document.

- [ ] **Step 3: Run targeted static/text checks and internal adversarial review**

Confirm no compatibility/fallback language, old metadata identity, canonical reappend, full-recorder distribution, raw-cap signature, or active accepted-before-sync claim remains.

- [ ] **Step 4: Run repository verification**

Run:

```bash
cargo fmt --check
cargo clippy --locked --features test-current-evidence-inspection --lib --bins -- -D warnings
cargo nextest run --locked --no-fail-fast --features test-current-evidence-inspection
cargo build --release --locked
```

Run equivalent BTE format, production-target clippy, nextest with only the named MinIO module skipped, and release build.

- [ ] **Step 5: Commit final generated/docs changes**

```bash
git add config src tests crates docs .github justfile Cargo.toml Cargo.lock
git commit -m "docs: finalize current evidence hard cutover"
```

- [ ] **Step 6: Verify clean diff and push**

```bash
git status --short
git diff --check d7a79229e7593f5a81940f30405db3f0dc2166a1...HEAD
git push
```

Expected: clean worktree and pushed exact head. Do not wait for CI; report the SHA and request the required native reviewer only after no local finding remains.

## Self-Review

- Spec coverage: tasks cover verification, path identity, capacity/frame bounds, cross-process ownership, producer domains, atomic admission, settlement equivalence, capability/lifecycle, poison publication, NT reconciliation ordering, shutdown, BTE, generated corpus, and docs.
- #1385 remains limited to rotation, total retained capacity, retirement, durable ordinals, and restart append-retry exact-once.
- No placeholder/TODO steps remain.
- The only runtime core is shared by live and BTE.
- The only state authority for sink health is the sink; publication tokens carry an edge, not state.
- The catalog lock is held on the already-authoritative directory descriptor; no lock file or second namespace is introduced.
- Current risk-reducing and forced-reduction facts cannot carry reservation attribution.
- All accepted durable prefixes are recoverable, inert, or typed fail-closed.
