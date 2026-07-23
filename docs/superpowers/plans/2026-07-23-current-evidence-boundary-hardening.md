# Current Decision-Evidence Boundary Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the six verified boundary defects in PR #1505 without adding historical compatibility, storage repair, alternate writers, or #1385-owned replay behavior.

**Architecture:** Replace file-only sinks with process-lifetime healthy/poisoned state machines, validate both retained streams through one sink-aware decoder, and replace pathname revalidation with one descriptor-relative Unix authority. Freeze the complete current wire acceptance domain with typed coverage cases and committed bytes, then align root/backtesting verification with the changed code.

**Tech Stack:** Rust 2024, `std::fs::File`, `libc::openat`/`fstatat`, serde/serde_json, Cargo nextest, GitHub Actions, TOML-generated decision-evidence contract.

## Global Constraints

- Preserve one current-only runtime path; no legacy codec, migration, fallback, repair, truncation, or alternate sink.
- `Rejected` occurs before I/O and does not poison; any write/sync error is commit-indeterminate and poisons exactly one sink.
- Observation content never grants or gates readiness; invalid retained observation content becomes an explicit poisoned observation sink.
- All active and retired filesystem operations use one retained descriptor-relative authority on macOS and Linux.
- TOML does not own wire variants or fixture cases; Rust frozen wire types and committed bytes remain authoritative.
- Tests verify behavior and bytes, never Rust source structure.
- #1385 continues to own durable ordinals, replay, exact-once, rotation, retirement, compaction, and capacity.

---

### Task 1: Process-Lifetime Sink Poisoning

**Files:**
- Modify/Test: `src/bolt_v3_current_evidence/record.rs`

**Interfaces:** Produces `CommitPhase`, `PoisonCause`, `RecordFailure::{CommitIndeterminate, SinkPoisoned}`, and healthy/poisoned sink construction. Existing purpose-specific recorder methods remain the only caller API.

- [ ] **Step 1: Write failing storage-state tests**

Add focused tests for partial write, post-write sync failure, later refusal, per-sink isolation, and rejection-before-I/O. The central assertions are:

```rust
assert!(matches!(first, Err(RecordFailure::CommitIndeterminate {
    phase: CommitPhase::Write,
    ..
})));
let retained = fs::read(&machine_path).unwrap();
assert!(matches!(retry, Err(RecordFailure::SinkPoisoned { .. })));
assert_eq!(fs::read(&machine_path).unwrap(), retained);
```

- [ ] **Step 2: Verify RED**

Run `cargo test --locked --lib bolt_v3_current_evidence::record::tests -- --nocapture`. Expect compilation/assertion failure because the state and variants do not exist.

- [ ] **Step 3: Implement the minimal sink state**

Introduce:

```rust
pub enum CommitPhase { Write, Sync }

pub enum PoisonCause {
    CommitIndeterminate { phase: CommitPhase, cause: Arc<str> },
    StartupContentInvalid { cause: Arc<str> },
}

enum DurableSinkState {
    Healthy(File),
    Poisoned(PoisonCause),
}
```

`RecordFailure::Rejected` leaves the state healthy. The first write/sync error stores its `PoisonCause` and returns `CommitIndeterminate`; later calls return `SinkPoisoned` without I/O. Extend the test-only fault mode with `PartialWrite(usize)`; production retains `write_all` followed by `sync_data`. Construct `AppendReceipt` only after both succeed.

- [ ] **Step 4: Verify GREEN and commit**

Run the focused command again, then commit `record.rs` as `fix: poison indeterminate evidence sinks`.

### Task 2: Sink-Aware Retained Stream Validation

**Files:**
- Modify: `src/bolt_v3_current_evidence/reader.rs`
- Modify: `src/bolt_v3_current_evidence/runtime.rs`
- Modify: `src/bolt_v3_current_evidence/record.rs`
- Modify: `src/bolt_v3_current_evidence/mod.rs`
- Test: `tests/bolt_v3_current_evidence_runtime.rs`

**Interfaces:** Consumes pre-poisoned sink construction from Task 1. Produces `validate_stream(file, expected_sink, max_bytes)`, `ObservationStreamStatus`, and `DecisionEvidenceRuntime::observation_stream_status()`.

- [ ] **Step 1: Write failing observation startup tests**

Add table cases for blank, torn, legacy/unknown identity, machine identity, malformed observation payload, and valid observation content. Invalid content must leave bytes unchanged, preserve machine recovery, produce typed poisoned status, and return `FailureReported` then `FailureSuppressed` on two observation attempts without changing bytes.

- [ ] **Step 2: Verify RED**

Run `cargo test --locked --test bolt_v3_current_evidence_runtime observation -- --nocapture`. Expect failures because observation content is not decoded and no status exists.

- [ ] **Step 3: Generalize validation**

Replace `validate_machine_stream` with:

```rust
pub(super) fn validate_stream(
    file: &mut File,
    expected_sink: KnownSink,
    max_bytes: Option<u64>,
) -> Result<ValidatedStream>
```

Use one framing/header/exact-identity/gate/full-decode loop. Require identity purpose sink equality. Apply startup recovery facts only for `KnownSink::Machine`; observation facts still fully decode.

- [ ] **Step 4: Construct pre-poisoned observation state**

Machine validation errors still abort runtime construction. Observation content errors preserve bytes, log once with `log::error!`, construct `PoisonCause::StartupContentInvalid`, and expose:

```rust
pub enum ObservationStreamStatus {
    Available,
    Poisoned { cause: Arc<str> },
}
```

Descriptor/path/alias failures remain startup errors.

- [ ] **Step 5: Verify GREEN and commit**

Run the focused integration test and reader tests. Commit the five files as `fix: validate retained observation evidence`.

### Task 3: Descriptor-Relative Filesystem Authority

**Files:**
- Create: `src/bolt_v3_current_evidence/path_authority.rs`
- Modify: `src/bolt_v3_current_evidence/mod.rs`
- Modify: `src/bolt_v3_current_evidence/runtime.rs`
- Test: `tests/bolt_v3_current_evidence_runtime.rs`

**Interfaces:** Produces internal `CatalogDirectory`, `open_stream(relative)`, and `ensure_retired_absent(relative)` operations backed only by retained descriptors. Existing `validate_relative_path` remains the lexical config fence.

- [ ] **Step 1: Write failing path tests**

Add Unix cases for an intermediate symlink on active and retired paths. Decompose the authority so a test can retain a parent descriptor, rename its directory, replace the old pathname with an outside symlink, and prove a final relative open remains inside the renamed original directory. Retain final-symlink, inode-alias, permission, and descriptor-retention tests.

- [ ] **Step 2: Verify RED**

Run `cargo test --locked --test bolt_v3_current_evidence_runtime path -- --nocapture`. Expect the parent replacement/intermediate symlink case to expose the pathname authority.

- [ ] **Step 3: Implement one component walk**

Hold the catalog as `OwnedFd`. Open each parent with `openat(O_RDONLY | O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC)`. Open/create the basename relative to that descriptor with `O_RDWR | O_APPEND | O_CREAT | O_NOFOLLOW | O_CLOEXEC` and private mode, then validate via `fstat`. Inspect retired basenames with `fstatat(..., AT_SYMLINK_NOFOLLOW)`. Convert raw descriptors immediately into owned Rust types.

On non-Unix targets, runtime construction returns an unsupported-target error before filesystem mutation.

- [ ] **Step 4: Delete the competing pathname authority**

Remove `resolve_active_path`, `open_retained_stream`, platform-specific no-follow open helpers, and pathname before/after metadata checks. Active and retired operations must call only `CatalogDirectory` after lexical validation.

- [ ] **Step 5: Verify GREEN and commit**

Run all `bolt_v3_current_evidence_runtime` tests. Commit as `fix: anchor evidence paths to directory descriptors`.

### Task 4: Exhaustive Current Wire Corpus

**Files:**
- Modify: `src/bolt_v3_current_evidence/codec.rs`
- Modify: `src/bolt_v3_current_evidence/codec/admission.rs`
- Modify: `src/bolt_v3_current_evidence/codec/basket_admission.rs`
- Modify: `src/bolt_v3_current_evidence/codec/entry_skip.rs`
- Modify: `src/bolt_v3_current_evidence/codec/exit.rs`
- Modify: `src/bolt_v3_current_evidence/codec/lifecycle.rs`
- Modify: `src/bolt_v3_current_evidence/codec/loss.rs`
- Modify: `src/bolt_v3_current_evidence/codec/order_intent.rs`
- Modify: `src/bolt_v3_current_evidence/codec/order_reject.rs`
- Modify: `src/bolt_v3_current_evidence/codec/requote.rs`
- Modify: `src/bolt_v3_current_evidence/codec/settlement.rs`
- Modify: `src/bolt_v3_current_evidence/codec/strategy_input.rs`
- Modify: `src/bolt_v3_current_evidence/codec/venue_truth.rs`
- Modify: the 27 existing identity files in `tests/fixtures/bolt_v3/current_evidence/positive/`, preserving their current filenames and identity mapping.
- Create: `tests/fixtures/bolt_v3/current_evidence/reject/unknown_identity.jsonl`
- Create: `tests/fixtures/bolt_v3/current_evidence/reject/unknown_enum.jsonl`
- Create: `tests/fixtures/bolt_v3/current_evidence/reject/wrong_gate.jsonl`
- Create: `tests/fixtures/bolt_v3/current_evidence/reject/wrong_sink.jsonl`
- Create: `tests/fixtures/bolt_v3/current_evidence/reject/extra_field.jsonl`
- Create: `tests/fixtures/bolt_v3/current_evidence/reject/torn_record.jsonl`

**Interfaces:** Produces typed per-identity coverage cases and ordered byte-exact corpora. Preserves private frozen DTOs, exact identity dispatch, and programmatic strictness mutations.

- [ ] **Step 1: Write a failing coverage test**

Change `positive_fixture(identity)` into a multi-line `positive_corpus(identity)`. Add typed coverage cases carrying encoded production bytes plus declared enum spelling and optional-state coverage:

```rust
enum OptionalWireState { Absent, Null, Present }

struct WireCoverageCase {
    name: &'static str,
    encoded: EncodedEvidenceRecord,
    enum_values: Vec<(&'static str, &'static str)>,
    optional_states: Vec<(&'static str, OptionalWireState)>,
}
```

For each identity, require the union to contain every frozen enum/tag branch and every admitted optional state, then compare case lines in order with the committed JSONL corpus.

- [ ] **Step 2: Verify RED**

Run `cargo test --locked --lib bolt_v3_current_evidence::codec::tests::current_identity_corpus_is_complete_byte_exact_and_strict -- --nocapture`. Expect missing-coverage failures against the existing one-line fixtures.

- [ ] **Step 3: Make frozen enum coverage exhaustive**

Extend existing `bidirectional_unit_enum!` and admission outcome macros to emit test-only coverage values from their variant lists. For manually defined enums, use no-wildcard exhaustive conversions from test case enums so a new frozen variant breaks compilation until assigned a case. Do not add TOML case rows or source parsing.

- [ ] **Step 4: Build linear typed cases and committed bytes**

For every identity, distribute variants and optional absent/null/present states across typed semantic facts rather than forming a Cartesian product. Encode only through production codecs and decode every case through `decode_current_fact`. Update each positive file to the ordered multi-line bytes.

Add raw rejection files for unknown identity/enum, wrong gate, wrong sink, extra field, and torn framing; retain programmatic missing/wrong-type mutations.

- [ ] **Step 5: Verify GREEN and commit**

Run all codec tests and commit codec modules plus fixtures as `test: freeze complete current evidence wire domains`.

### Task 5: Semantic Migration of Stale Assertions

**Files:**
- Modify: `src/bolt_v3_live_node/risk_admission_loss.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/tests/core_glue.rs`

**Interfaces:** Consumes typed failure categories from Task 1. Preserves zero-submit, fail-closed, risk-reducing continuation, and exact evidence emission.

- [ ] **Step 1: Run the three known tests and confirm RED**

Run the two CI failures plus `decision_evidence_failure_rejects_before_nt_submit`. Confirm the failures are whole-stream assumptions or removed fake literals.

- [ ] **Step 2: Replace whole-stream assumptions with phase checkpoints**

Record fact count after fixture setup. Assert only the suffix emitted by the action under test, including exact fact kinds, while separately retaining zero-submit assertions. Do not discard legitimate `CapitalAdmissionRebuild` setup evidence.

- [ ] **Step 3: Assert typed/stable failure semantics**

Assert stable production operation context and `CommitIndeterminate`/`SinkPoisoned` category, never injected leaf strings such as `decision evidence unavailable` or `intent write failed`.

- [ ] **Step 4: Verify focused tests GREEN**

Run the three tests again and require all domain assertions to pass.

- [ ] **Step 5: Enumerate the class with a complete root run**

Run `cargo nextest run --locked --no-fail-fast`. Any newly exposed failure stops this task for root-cause diagnosis and a plan amendment naming its exact file; do not silently broaden this task.

- [ ] **Step 6: Commit**

Commit migrated tests as `test: assert current evidence semantics`.

### Task 6: Backtesting Advisory Coverage

**Files:**
- Modify: `.github/workflows/advisory.yml`
- Update externally: stable body of PR #1505.

**Interfaces:** Produces authoritative exact-head clippy/test/build evidence for the separate backtesting workspace.

- [ ] **Step 1: Extend existing advisory jobs**

Add nested-workspace steps to the existing clippy, test, and build jobs:

```yaml
- name: clippy (backtesting vertical slice)
  working-directory: crates/backtesting-vertical-slice
  run: just clippy

- name: test (backtesting vertical slice)
  working-directory: crates/backtesting-vertical-slice
  run: just test --no-fail-fast

- name: build (backtesting vertical slice)
  working-directory: crates/backtesting-vertical-slice
  run: just build
```

Keep formatting in its existing job. Do not create a second workflow or verification manifest.

- [ ] **Step 2: Validate and commit**

Run formatting and `git diff --check`, inspect the workflow diff, and commit as `ci: verify backtesting evidence integration`.

- [ ] **Step 3: Correct PR metadata**

Replace the false coverage sentence with the stable fact that advisory CI explicitly runs formatting, clippy, unfiltered tests, and release build for the nested workspace.

### Task 7: Exact-Head Verification and Handoff

**Files:**
- Modify: `docs/runbooks/current-decision-evidence-hard-cutover.md`
- Modify: `docs/superpowers/specs/2026-07-22-current-decision-evidence-rebuild-design.md`
- Update externally: PR #1505 exact-head evidence comment.

**Interfaces:** Produces review evidence only; it does not merge or authorize live cutover.

- [ ] **Step 1: Update active operational documentation**

State that torn/indeterminate machine state remains fail-closed and requires pause/archive/forward-fix. State that corrupt observation content is retained, visibly poisons only its sink, and never supplies readiness authority.

- [ ] **Step 2: Run cheap local verification**

Run `cargo fmt --check`, `cargo fmt --check --manifest-path crates/backtesting-vertical-slice/Cargo.toml`, and `git diff --check`.

- [ ] **Step 3: Run focused boundary tests**

Run recorder, runtime, codec corpus, and migrated semantic tests. Focused results do not substitute for advisory exact-head evidence.

- [ ] **Step 4: Commit documentation and deterministic outputs**

Commit as `docs: record hardened evidence boundaries`; skip only if the worktree is already clean.

- [ ] **Step 5: Push and detach**

Run plain `git push`, report the exact pushed SHA, and do not wait locally for CI.

- [ ] **Step 6: Request required review only after findings are resolved**

Confirm a clean pushed worktree and answered review threads. Resolve the login for required reviewer node ID `U_kgDOEZMFhA`, request that reviewer, and do not merge.
