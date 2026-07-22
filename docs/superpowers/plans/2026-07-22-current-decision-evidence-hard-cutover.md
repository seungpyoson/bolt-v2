# Current Decision-Evidence Hard Cutover Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the released global-v15 decision-evidence lane with one current-only purpose contract, separate machine and observation streams, and a fail-closed archival cutover that contains no historical runtime decoder or migration path.

**Architecture:** Every structural producer maps to one purpose, current identity, private DTO, sink, and effect policy. Startup validates the complete current machine stream before activation; exact current identities decode into typed facts and exhaustive consumer dispositions. Pre-cutover files are archived outside the active data root and are never runtime inputs.

**Tech Stack:** Rust, serde/serde_json, TOML, generated exhaustive Rust, JSONL, existing Nautilus/Polymarket provider and recovery APIs.

## Global Constraints

- The branch starts from current `main`; the prior 43-commit branch is read-only donor material.
- No historical identity rows, released-epoch DTOs, legacy codecs, compatibility adapters, migration commands, or fallback decoding may enter the branch.
- Every runtime path, cap, sink, identity, and cutover location comes from validated TOML.
- The trading binary never deletes, archives, truncates, repairs, or migrates evidence.
- Old, unknown, mixed, malformed, torn, or oversized machine streams fail before live activation.
- Recovery-bearing records are never novelty-suppressed; only registered observations can receive novelty capability.
- Archive, do not delete, pre-cutover evidence. Archived bytes are inert and cannot be restored to an active path.
- Live activation remains prohibited until authoritative external state proves no fill, settlement, redemption, reservation, order, or position remains in flight.
- Tests verify behavior or compiler-enforced contracts, never Rust source appearance.
- Evidence-driven verification applies; focused local checks supplement rather than replace exact-head advisory CI.

---

### Task 1: Current-Only Purpose Contract And Generator

**Files:**
- Create: `config/decision-evidence-contract.toml`
- Create: `src/bolt_v3_decision_evidence/contract_generator.rs`
- Create: `src/bolt_v3_decision_evidence/generated_contract.rs`
- Create: `src/bin/generate_decision_evidence_contract.rs`
- Create: `tests/bolt_v3_decision_evidence_contract.rs`
- Modify: `tests/wiring_registration.rs`

**Interfaces:**
- Produces: sealed `KnownProducer`, `KnownPurpose`, `KnownIdentity`, `KnownSink`, `KnownDecodedFact`, `KnownConsumer`, `EffectPolicy`, and total `ConsumerDisposition` functions.
- Produces: `resolve_identity(kind: &str, schema_version: u32) -> Result<KnownIdentity>` matching current identities only.

- [ ] **Step 1: Port only current registry rows**

Use the donor registry as evidence, then remove every `status = "historical"` identity, provenance field, compatibility baseline, and dormant replace-admission purpose. Every retained purpose must have one current identity, one fact, one sink, one effect policy, and explicit consumer dispositions.

- [ ] **Step 2: Port deterministic generation**

Generate sealed marker types and exhaustive matches without `_`, complement-derived irrelevance, optional routes, string-based Rust function paths, or ordered-version comparisons. Registry parsing uses `deny_unknown_fields`; adding a producer, purpose, fact, identity, or consumer leaves the relation incomplete and rejects generation.

- [ ] **Step 3: Add current-only rejection tests**

Tests must reject duplicate allocations, missing current encoders, missing disposition cells, novelty capability on recovery facts, any historical status, and any exact identity not present in the current registry.

- [ ] **Step 4: Prove deterministic output**

Run the generator twice and compare `src/bolt_v3_decision_evidence/generated_contract.rs` byte-for-byte. Run the focused registry suite and `cargo fmt --check`.

- [ ] **Step 5: Commit**

Commit message: `feat(#1354): register current evidence purposes`

### Task 2: Current Facts, Private DTOs, Split Sinks, And Startup Validation

**Files:**
- Create: `src/bolt_v3_decision_evidence/facts.rs`
- Create: `src/bolt_v3_decision_evidence/current.rs`
- Create: `src/bolt_v3_decision_evidence/current/*.rs`
- Create: `src/bolt_v3_decision_evidence/sink.rs`
- Create: `src/bolt_v3_decision_evidence/stream.rs`
- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_validate/persistence.rs`
- Modify: `config/root.toml`
- Modify: `tests/fixtures/bolt_v3/root.toml`
- Modify: `tests/config_parsing.rs`
- Modify: `tests/bolt_v3_decision_evidence_contract.rs`

**Interfaces:**
- Produces: `EncodedEvidenceRecord`, `DecisionEvidenceSink`, `AppendReceipt`, and `RecordError::{Rejected, AppendFailed}`.
- Produces: `decode_current_line(&[u8]) -> Result<DecodedFact>` and `validate_current_machine_stream(path, max_bytes) -> Result<()>`.

- [ ] **Step 1: Add validated stream configuration**

Replace `order_intents_relative_path` with required, distinct `machine_relative_path` and `observation_relative_path`, plus configured retired paths. Reject empty, absolute, parent-traversing, equal, symlinked, or out-of-root paths.

- [ ] **Step 2: Port neutral facts and current DTOs**

Move semantic fact types into `facts.rs`. Port only current codecs. Each top-level DTO spells out the envelope fields, uses `deny_unknown_fields`, binds one current identity, and validates its full enum/value domain before producing a fact.

- [ ] **Step 3: Port the durable split sink**

Only current encoders construct `EncodedEvidenceRecord`; only the JSONL sink constructs `AppendReceipt` after `sync_data`. Machine and observation records resolve their sink from the generated purpose contract.

- [ ] **Step 4: Implement unconditional machine-stream validation**

Before recovery or writer construction, reject configured retired-path presence, non-regular/symlink paths, over-cap streams, torn lines, old identities, unknown identities, malformed current payloads, and mixed content. An absent or empty fresh machine stream is valid; the observation stream is never recovery-read.

- [ ] **Step 5: Add behavior fixtures**

Add byte-exact positive fixtures and negative missing-field, wrong-type, unknown-field, wrong-gate, unknown-enum, old-identity, torn-line, exact-cap, one-byte-over, and retired-path tests for every machine fact and representative observations.

- [ ] **Step 6: Verify and commit**

Run config, registry, sink, codec, and stream-validation suites. Commit message: `feat(#1354): add current evidence streams`

### Task 3: Atomic Producer And Consumer Cutover

**Files:**
- Modify: `src/bolt_v3_decision_evidence.rs`
- Create: `src/bolt_v3_decision_evidence/consumers.rs`
- Modify: `src/bolt_v3_submit_admission.rs`
- Modify: `src/bolt_v3_basket_execution.rs`
- Modify: `src/bolt_v3_live_node.rs`
- Modify: `src/bolt_v3_live_node/risk_admission_loss.rs`
- Modify: `src/bolt_v3_settlement_booking.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/shadow_pnl.rs`
- Modify: production and test call sites identified by the producer/reader census.

**Interfaces:**
- Consumes: generated producer/purpose markers, current encoders, split sink, and facts.
- Produces: one production writer path and one generated consumer runner; removes the released v15 writer/reader lane.

- [ ] **Step 1: Replace the wide writer trait**

Expose purpose-specific record commands requiring generated producer tokens and typed inputs. Remove default no-op methods, generic configured-error methods, schema-v15 line encoders, and any path that can stamp an identity outside the generator.

- [ ] **Step 2: Route every producer explicitly**

Blocked observations and submit-linked snapshots use different identities and files. Entry/exit intents, admissions, reservations, settlements, lifecycle, safety, and observation producers each use their registered current identity and effect policy.

- [ ] **Step 3: Preserve effect-policy behavior**

New-risk actions require durable append first; risk-reducing actions continue while surfacing append failure; reconciliation failures enter unreconciled state; observations use bounded failure reporting. No typed outcome is collapsed to `Result<()>` where callers distinguish it.

- [ ] **Step 4: Replace all readers**

One consumer runner validates the complete machine stream, decodes exact identities, resolves explicit fact×consumer dispositions, and feeds typed event enums. Delete target-kind generic readers, version-order predicates, query readers with no production caller, and Shadow PnL's private evidence parser.

- [ ] **Step 5: Add composition and restart tests**

Test reservation/fill restart, settlement/booking/terminal permutations, flat restart still scanning foreign content, multiple blocked observations in Shadow PnL, malformed relevant failure, registered irrelevant observation isolation, and observation floods leaving machine bytes unchanged.

- [ ] **Step 6: Verify and commit**

Run focused strategy, admission, settlement, restart, Shadow PnL, and decision-evidence suites. Commit message: `feat(#1354): cut over current decision evidence`

### Task 4: Observation Novelty And Hard-Cutover Operations

**Files:**
- Create or modify: current observation novelty modules under `src/bolt_v3_decision_evidence/`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Delete: `scripts/migrate_bolt_v3_decision_evidence_to_v15.py`
- Delete: its dedicated tests and references.
- Create: `docs/operations/decision-evidence-hard-cutover.md`
- Modify: tests for novelty and operations behavior.

**Interfaces:**
- Produces: typed observation-only monotone novelty and the operator cutover contract.
- Does not produce: a runtime archive/delete/migrate command or persisted readiness authority.

- [ ] **Step 1: Port observation-only novelty**

Use typed episode IDs and complete semantic keys with monotone no-eviction sets. Identical state emits once; A→B→A emits twice; more than 4,096 episodes do not forget; recovery facts cannot construct novelty capability.

- [ ] **Step 2: Delete migration and compatibility tooling**

Remove the Python migrator and every invocation. Do not replace it with another migration path.

- [ ] **Step 3: Write the archival runbook**

Specify stop/mask, immutable binary/config staging, authoritative consecutive venue/account reads, zero orders/positions/redemptions/reservations/settlements, kill-switch disposition, same-filesystem archive rename, checksum/fsync/read-only retention, fresh configured paths, and pause/forward-fix after the first current machine line.

- [ ] **Step 4: State the activation blocker precisely**

The runbook must state that current provider evidence does not independently prove the fill/settlement cutoff. Live cutover is prohibited until an operator can supply that proof; ordinary startup never infers or persists it.

- [ ] **Step 5: Verify and commit**

Run novelty behavior tests, migration-reference checks, `cargo fmt --check`, and `git diff --check`. Commit message: `fix(#1354): bound current observation evidence`

### Task 5: Exact-Head Evidence And PR Supersession

**Files:**
- Modify only files required to resolve local findings.
- Produce the external review request in the handoff, not as mutable PR-body status.

**Interfaces:**
- Produces: a clean pushed prerequisite PR from current main.

- [ ] **Step 1: Run local evidence**

Run formatting, deterministic generation, focused contract/codec/recovery/novelty suites, and economically reasonable checks. Record commands and exact results.

- [ ] **Step 2: Conduct internal adversarial review**

Re-audit historical/legacy names, version ordering, wildcard routing, default writer methods, parser duplication, shared stream paths, outcome erasure, observation-to-machine routing, and all production producer/consumer call sites.

- [ ] **Step 3: Confirm clean exact head**

Run `git diff --check`, `git status --short`, and `git log -1 --oneline`. Resolve every local finding before publishing.

- [ ] **Step 4: Publish without merging**

Push the branch, open a new prerequisite PR, link and supersede #1470/#1475/#1476/#1478 without merging or closing them automatically, report the exact SHA, and detach without waiting for advisory CI.

- [ ] **Step 5: Prepare review evidence**

Request exact-head review of producer completeness, current-only identity closure, sink separation, startup validation, typed effect policies, recovery behavior, novelty behavior, absence of compatibility paths, and the explicit live-activation blocker.
