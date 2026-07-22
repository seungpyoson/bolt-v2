# Current Decision-Evidence Rebuild Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the existing decision-evidence implementation with one current-only, compiler-closed hard-cutover runtime that cannot bypass startup validation, counterfeit durable append, share mutable wire identities, or flood failure logs.

**Architecture:** Build a new `bolt_v3_current_evidence` module from current `main`; do not edit the old runtime into the desired shape. A single runtime constructor retains the machine descriptor it validates, exposes one concrete recorder with policy-specific outcomes, and supplies typed recovery projections. Each exact identity owns a private V1 line/payload codec; the old module and paths are deleted only when every producer and consumer has moved.

**Tech Stack:** Rust 2024, serde/serde_json, TOML registry generation, existing `anyhow`, Unix `O_NOFOLLOW`, current Bolt configuration and test infrastructure.

## Global Constraints

- Follow repository rules: NO HARDCODES, NO DUAL PATHS, NO DEBTS, PURE RUST BINARY, and behavior/compiler tests rather than source-scanning tests.
- Work only in `.worktrees/1354-current-evidence-rebuild` on `codex/1354-current-evidence-rebuild`.
- Do not port PR #1503 runtime modules or its transparent runtime-wire wrappers.
- Pre-cutover bytes are unsupported runtime input and must fail closed.
- No implementation checkpoint is complete until its named red/green evidence exists.
- Local Rust builds use `CARGO_PROFILE_DEV_DEBUG=0` and `CARGO_PROFILE_TEST_DEBUG=0` because normal debug artifacts exhaust the workstation disk; exact-head advisory CI remains the authoritative full-build evidence.
- Live activation remains prohibited; #1385 retains capacity/rotation/exact-once scope.

---

### Task 1: Closed Current Contract

**Files:**
- Create: `config/decision-evidence-contract.toml`
- Create: `src/bolt_v3_current_evidence/mod.rs`
- Create: `src/bolt_v3_current_evidence/contract_generator.rs`
- Create: `src/bolt_v3_current_evidence/generated_contract.rs`
- Create: `src/bin/generate_decision_evidence_contract.rs`
- Modify: `src/lib.rs`
- Test: `tests/bolt_v3_current_evidence_contract.rs`

**Interfaces:**
- Produces: `KnownProducer`, `KnownPurpose`, `KnownIdentity`, `KnownFact`, `KnownConsumer`, `KnownSink`, `EffectPolicy`, `current_identity_for_purpose`, `sink_for_purpose`, `effect_policy_for_purpose`, and exhaustive fact-consumer dispositions.
- Consumes: only TOML registry IDs and metadata; no Rust function-name strings.

- [ ] **Step 1: Write failing registry closure tests**

Add behavior tests that parse the registry and assert rejection after independently removing a producer, current identity, fact-consumer cell, owner, sink, or effect policy; adding a consumer must invalidate every unadjudicated fact row. Assert duplicate exact `(kind, schema_version)` pairs and observation-to-machine routing reject.

Run:

```bash
CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --test bolt_v3_current_evidence_contract
```

Expected: compilation fails because `bolt_v3_current_evidence` and the parser do not exist.

- [ ] **Step 2: Implement the registry parser and validator**

Use closed serde rows with `#[serde(deny_unknown_fields)]`. Register these current purposes, each with exactly one structural producer and fresh identity:

```text
blocked_strategy_input_observation
submit_linked_strategy_input_snapshot
entry_order_intent
risk_reducing_exit_order_intent
admitted_entry_admission
rejected_entry_admission
risk_reducing_exit_admission
forced_reduction_admission
basket_admission_granted
basket_admission_rejected
capital_admission_rebuild
submit_reservation_metadata
submit_reservation_fill
entry_skip_observation
exit_submission_decision
exit_hold_decision
exit_evaluation
loss_governor_halt
order_reject
order_lifecycle
requote_throttle_observation
settlement
settlement_booking_error
terminal_settlement
venue_truth_capture_failure
venue_truth_divergence
```

Machine duties are `action`, `join`, `reconciliation`, or `recovery`. Observation duties are `state_observation` or `diagnostic_observation`. Generator validation must make those duty sets disjoint and reject observation purposes using a machine sink or non-observation failure policy.

- [ ] **Step 3: Generate sealed Rust markers and exhaustive relations**

Generate enums deriving `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash`. Generate total matches without wildcard arms. Bind each purpose to one identity, sink, effect policy, fact, and producer. Bind every fact-consumer cell to either a typed event variant or an explicit owner-ruling ID.

- [ ] **Step 4: Verify deterministic generation**

Run the focused test twice and compare a fresh render with the committed bytes. Expected: PASS with byte-identical output.

- [ ] **Step 5: Commit the closed contract**

```bash
git add config/decision-evidence-contract.toml src/bolt_v3_current_evidence src/bin/generate_decision_evidence_contract.rs src/lib.rs tests/bolt_v3_current_evidence_contract.rs
git commit -m "feat(#1354): close current evidence contract"
```

### Task 2: Atomic Runtime and Typed Durability

**Files:**
- Create: `src/bolt_v3_current_evidence/runtime.rs`
- Create: `src/bolt_v3_current_evidence/record.rs`
- Create: `src/bolt_v3_current_evidence/reader.rs`
- Modify: `src/bolt_v3_current_evidence/mod.rs`
- Modify: `src/bolt_v3_config.rs`
- Modify: `config/root.toml`
- Modify: `tests/fixtures/bolt_v3/root.toml`
- Modify: `tests/config_parsing.rs`
- Test: `tests/bolt_v3_current_evidence_runtime.rs`

**Interfaces:**
- Produces: `DecisionEvidenceRuntime::open`, `DecisionEvidenceRecorder`, `StartupRecoveryFacts`, `AppendReceipt`, `RecordFailure`, `NonBlockingRecordOutcome`, and `ObservationRecordOutcome`.
- Consumes: generated purpose/sink/effect metadata from Task 1.

- [ ] **Step 1: Write failing atomic-open behavior tests**

Tests must cover missing fresh machine path, current empty file, retired-path presence, symlink, directory/non-regular path, machine/observation path equality, hard-link inode alias, torn line, blank line, old identity, unknown identity, observation identity in machine stream, exact byte cap, and one byte over. Add a replacement-race harness proving appends use the descriptor validated by `open`, not a later path occupant.

Expected RED: no `DecisionEvidenceRuntime` exists.

- [ ] **Step 2: Replace the config schema**

Replace `order_intents_relative_path` with required `machine_relative_path`, `observation_relative_path`, and `retired_relative_paths`. Preserve `recovery_evidence_max_bytes`. Validate every relative path beneath `catalog_directory`, reject empty/absolute/parent traversal, and reject duplicate configured paths.

- [ ] **Step 3: Implement the only runtime constructor**

Expose exactly:

```rust
pub struct DecisionEvidenceRuntime {
    recorder: Arc<DecisionEvidenceRecorder>,
    startup_recovery: StartupRecoveryFacts,
}

impl DecisionEvidenceRuntime {
    pub fn open(config: &LoadedBoltV3Config) -> Result<Self>;
    pub fn recorder(&self) -> Arc<DecisionEvidenceRecorder>;
    pub fn startup_recovery(&self) -> &StartupRecoveryFacts;
}
```

`open` must open the machine descriptor with read+append+create+no-follow, validate and decode from that descriptor, seek it to append, then retain it. Do not expose a writer constructor or standalone preflight.

- [ ] **Step 4: Write failing durability and failure-episode tests**

Inject write failure and sync failure below the recorder. Assert neither yields `AppendReceipt`. Assert the first observation failure for a purpose is `FailureReported`, the second is `FailureSuppressed`, successful append resets the episode, and a later failure reports once again.

- [ ] **Step 5: Implement policy-specific outcomes**

Use these exact public outcomes:

```rust
pub struct AppendReceipt { purpose: KnownPurpose, sink: KnownSink, bytes: usize }

pub enum RecordFailure {
    Rejected(anyhow::Error),
    AppendFailed(anyhow::Error),
}

pub enum NonBlockingRecordOutcome {
    Appended(AppendReceipt),
    Failed(RecordFailure),
}

pub enum ObservationRecordOutcome {
    Appended(AppendReceipt),
    FailureReported(RecordFailure),
    FailureSuppressed,
}
```

Only the private durable append function may construct `AppendReceipt`, after `write_all` and `sync_data`. The recorder owns `Mutex<BTreeSet<KnownPurpose>>` for continuous observation failure episodes and clears a purpose on success.

- [ ] **Step 6: Verify and commit the runtime boundary**

Run the focused runtime and config tests with debug info disabled. Expected: PASS.

```bash
git add src/bolt_v3_current_evidence src/bolt_v3_config.rs config/root.toml tests/fixtures/bolt_v3/root.toml tests/config_parsing.rs tests/bolt_v3_current_evidence_runtime.rs
git commit -m "feat(#1354): seal current evidence runtime"
```

### Task 3: Identity-Owned Frozen Codecs

**Files:**
- Create: `src/bolt_v3_current_evidence/facts.rs`
- Create: `src/bolt_v3_current_evidence/codec/mod.rs`
- Create: `src/bolt_v3_current_evidence/codec/strategy_input.rs`
- Create: `src/bolt_v3_current_evidence/codec/order_intent.rs`
- Create: `src/bolt_v3_current_evidence/codec/admission.rs`
- Create: `src/bolt_v3_current_evidence/codec/basket_admission.rs`
- Create: `src/bolt_v3_current_evidence/codec/reservation.rs`
- Create: `src/bolt_v3_current_evidence/codec/entry_skip.rs`
- Create: `src/bolt_v3_current_evidence/codec/exit.rs`
- Create: `src/bolt_v3_current_evidence/codec/lifecycle.rs`
- Create: `src/bolt_v3_current_evidence/codec/settlement.rs`
- Create: `src/bolt_v3_current_evidence/codec/venue_truth.rs`
- Create: `tests/fixtures/bolt_v3/current_evidence/positive/*.jsonl`
- Create: `tests/fixtures/bolt_v3/current_evidence/reject/*.jsonl`
- Test: `tests/bolt_v3_current_evidence_codec.rs`

**Interfaces:**
- Produces: `EncodedEvidenceRecord`, `DecodedFact`, identity-specific `CodecFor<IdentityMarker>` implementations, and typed consumer events.
- Consumes: semantic input/fact values and generated identity markers.

- [ ] **Step 1: Write the failing codec conformance harness**

For every current identity, require byte-exact encode/decode fixtures. For every admitted payload, remove each required field and substitute a wrong JSON type; both must reject. Cover every frozen enum variant exhaustively and every optional field's admitted absent/null/present states. Reject unknown envelope/payload fields, wrong gate, wrong exact pair, unknown enum, and contradictory semantic combinations.

Expected RED: identity codec bindings and fixtures do not exist.

- [ ] **Step 2: Define neutral semantic facts**

Move the producer/consumer semantic vocabulary out of the old evidence module into `facts.rs`. These types do not derive serde solely for persistence. Runtime-only IDs and enums remain semantic types; frozen codecs convert them explicitly.

- [ ] **Step 3: Implement dedicated identity modules**

Within each domain file, define a distinct private `LineV1` and `PayloadV1` for every identity in that domain. Blocked/submit and exit-submit/exit-hold must have separate top-level payload structs even when fields match. Frozen V1 enums must not alias or contain serde-derived runtime enums.

Each binding has this shape:

```rust
impl CodecFor<generated_contract::SubmitReservationMetadataV1> for CurrentCodecs {
    type Input = SubmitReservationMetadataFact;
    type Fact = SubmitReservationMetadataFact;

    fn encode(input: &Self::Input, recorded_at_utc_ns: i64) -> Result<EncodedEvidenceRecord>;
    fn decode(line: &[u8]) -> Result<Self::Fact>;
}
```

Conversions use exhaustive `TryFrom`; no `serde_json::Value` projection is allowed.

- [ ] **Step 4: Complete raw positive and rejection corpora**

Fixtures must originate from valid semantic builders, then be committed as raw bytes. The entry-skip unknown-reason case serializes `unclassified` with its context rather than inventing an enum variant. Machine facts receive exhaustive field/domain coverage; observations receive every enum branch and representative boundary/optionality coverage declared by their codec tests.

- [ ] **Step 5: Verify and commit codecs**

Run codec and contract tests. Expected: PASS, including deterministic bytes and exhaustive conversions.

```bash
git add src/bolt_v3_current_evidence tests/fixtures/bolt_v3/current_evidence tests/bolt_v3_current_evidence_codec.rs
git commit -m "feat(#1354): add frozen current evidence codecs"
```

### Task 4: Producer and Consumer Cutover

**Files:**
- Modify: every production call site currently invoking `BoltV3DecisionEvidenceWriter`
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `src/bolt_v3_settlement_booking.rs`
- Modify: `src/shadow_pnl.rs`
- Modify: strategy/runtime contexts that store the writer
- Test: existing producer, admission, recovery, settlement, strategy, and Shadow-PnL tests
- Test: `tests/bolt_v3_current_evidence_integration.rs`

**Interfaces:**
- Consumes: `DecisionEvidenceRuntime`, concrete recorder, frozen codecs, startup recovery facts, and generated consumer routes.
- Produces: one production write/read path with no old writer or reader invocation.

- [ ] **Step 1: Write failing vertical integration tests**

Assert each registered producer emits exactly one exact identity to its configured sink. Assert new-risk failure blocks the action; risk reduction proceeds with `Failed`; reconciliation failure enters the existing unreconciled path; observations preserve trading results and report only the first continuous failure.

- [ ] **Step 2: Construct one runtime during live-node startup**

Replace conditional/no-op writer construction with one `DecisionEvidenceRuntime::open`. Pass cloned concrete recorder handles to producers. Feed reservation and settlement startup from `startup_recovery()` rather than reopening the path.

- [ ] **Step 3: Rewire every producer purpose**

Split shared producer calls by semantic purpose before encoding. In particular, blocked strategy snapshots cannot call the submit-linked method; entry and exit intents/admissions cannot select identity from payload fields; exit hold and exit submission use distinct methods.

Every call site must exhaustively handle its policy-specific return type. Remove duplicate caller error logs for observation failures because the recorder owns the bounded episode report.

- [ ] **Step 4: Rewire consumers to typed events**

Reservation recovery, unified settlement recovery, entry-chain analysis, and Shadow PnL consume generated typed routes over `DecodedFact`. Known irrelevant observations skip only after exact envelope validation; malformed relevant machine records fail closed.

- [ ] **Step 5: Verify restart and separation behavior**

Test current-only reservation/fill restart, settlement/booking-error/terminal reconstruction in all relevant orders, blocked observations excluded from entry/shadow joins, and an observation flood leaving machine bytes and recovery output unchanged.

- [ ] **Step 6: Commit the cutover**

Run all focused producer and consumer suites. Expected: PASS.

```bash
git add src tests
git commit -m "feat(#1354): cut over current evidence producers and consumers"
```

### Task 5: Delete the Retired Path and Prove One Authority

**Files:**
- Delete: old runtime implementation from `src/bolt_v3_decision_evidence.rs` after semantic types have moved
- Delete: `scripts/migrate_bolt_v3_decision_evidence_to_v15.py`
- Delete: its tests and old-format fixtures
- Modify: `src/lib.rs`
- Modify: imports throughout `src/` and `tests/`
- Create: `docs/runbooks/current-decision-evidence-hard-cutover.md`

**Interfaces:**
- Produces: one current evidence module and an operator-only archival runbook.
- Consumes: fully cut-over production paths from Task 4.

- [ ] **Step 1: Delete the old write/read/migration authority**

Remove schema-v15 encoders, the wide/default writer trait, generic target-kind readers, version-order predicates, dead query readers, Shadow PnL's private parser, and the Python migration path. Retain no compatibility adapter or fallback.

- [ ] **Step 2: Add the hard-cutover runbook**

Document stop/mask, independent venue/account quiescence, archive-by-rename with checksum/fsync/read-only retention, fresh configured paths, first-boot verification, and pause/forward-fix after the first current machine record. State explicitly that the repository does not yet provide the independent fill/settlement barrier needed for live authorization.

- [ ] **Step 3: Verify no behavioral dual path remains**

Use compilation and behavior tests, not a Rust source-scanning test. Build all targets so stale imports/callers fail. Exercise the single live startup and offline reader entry points.

- [ ] **Step 4: Commit deletion**

```bash
git add -A
git commit -m "refactor(#1354): retire legacy decision evidence path"
```

### Task 6: Exact-Head Verification and Review

**Files:**
- Modify only files required by verified failures; no scope expansion.

**Interfaces:**
- Produces: exact-head evidence for replacement PR publication.
- Consumes: completed Tasks 1-5.

- [ ] **Step 1: Run cheap local checks**

```bash
cargo fmt --check
git diff --check origin/main...HEAD
just source-fence
```

Expected: all succeed.

- [ ] **Step 2: Run focused local Rust checks with compact artifacts**

```bash
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --test bolt_v3_current_evidence_contract
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --test bolt_v3_current_evidence_runtime
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --test bolt_v3_current_evidence_codec
CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo test --locked --test bolt_v3_current_evidence_integration
```

Expected: all succeed. If disk prevents a check, record that environmental blocker and rely on exact-head advisory CI rather than claiming a local pass.

- [ ] **Step 3: Conduct an internal adversarial review**

Review exact-head startup authority, receipt construction, identity ownership, fact-consumer totality, observation bounds, remaining call sites, and claimed fixture coverage. Resolve every substantive finding before publication.

- [ ] **Step 4: Push and open the replacement draft PR**

Push the exact branch head with plain `git push`, open a draft PR scoped to the #1354 current-only cutover, report its SHA, and detach without waiting for CI.

- [ ] **Step 5: Supersede rejected PRs only after publication**

After the replacement PR exists, close #1470 and #1503 with links to the replacement. Request the required reviewer only with a clean worktree, pushed commits, resolved local findings, and no unanswered review threads.
