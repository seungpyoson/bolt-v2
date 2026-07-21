# PR #1470 Complete Semantic Novelty Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Subagent execution is not authorized for this task.

**Goal:** Make PR #1470 emit each approved non-recovery evidence state once per stable market episode while preserving every distinct typed semantic transition and failing closed on unknown identity or state.

**Architecture:** Replace the numeric whole-state bitmap with `EvidenceEpisodeId -> BTreeSet<CompleteTypedSemanticKey>`. TOML owns producer registration, ordered key dimensions, closed leaf domains, numeric dimension IDs, allocation, and producer ownership; a Rust generator emits sealed producer markers, complete key structs, and exhaustive bindings to the existing typed evidence enums. Runtime producers construct one generated key and call one at-most-one-attempt API; no Python source parsing, handwritten blocker dispatch, stringly semantic state, generic fallback, or cross-product state enumeration remains.

**Tech Stack:** Rust 2024, `serde`, `toml`, `BTreeMap`, `BTreeSet`, existing Bolt decision-evidence types, Cargo integration tests, governed GitHub Rust Probe/Advisory evidence.

## Global Constraints

- Follow repository `AGENTS.md`; current `main` and issue #1354's 2026-07-15 amendment are authoritative.
- Keep submit-linked and every other recovery-bearing evidence append unsuppressed.
- Do not change PRs #1475, #1476, #1478, exit/risk novelty owned by #1385, backtesting, capture, writer storage, rotation, recovery limits, or evidence schemas.
- Do not add conditional fallbacks, dual paths, runtime string keys, runtime hardcodes, Python runtime/source parsing, source-structure tests, or an eviction ceiling.
- Mark before invoking an evidence writer. A writer failure remains attempted and cannot create a retry flood; the outcome must never claim durable append success.
- Verification is remote-first. Run governed non-compile preflight locally, then exact-head affected Rust tests remotely without Final Review.

---

## External Design-Review Adjudication

### Accepted for #1470

1. **Complete producer-specific keys.** The three failures prove that category-only and RV-gate-only state is incomplete. Entry keys retain reason, gate blocker set, pricing blocker set with payload association, fast/reference availability, incoherence, RV gate, and watermark presence. Blocked-RV keys additionally retain selected side, failover state, typed RV blockers, and canonical typed per-source status.
2. **Generic monotone guard.** Use `EvidenceNoveltyGuard<P>` where sealed `P: NoveltyEligibleProducer` supplies one generated `Key: Ord`. Storage is `BTreeMap<EvidenceEpisodeId, BTreeSet<P::Key>>` with no eviction.
3. **TOML/Rust authority boundary.** TOML schema v2 declares producers, dimensions, closed variants, dimension IDs, allocation, and owner. Existing Rust evidence enums carry runtime values. Generated exhaustive matches prove the TOML leaf list and Rust enum variants agree; generated key fields prove every declared dimension is supplied.
4. **Structured injectivity.** Keys store typed enum values, including enum payloads. They never flatten `{kind, side}` into independent bits, so swapped payload associations remain distinct.
5. **Canonical collections.** `CanonicalSet<T>::try_from_iter` sorts values and rejects duplicates. `CanonicalSourceStates` validates a unique registered source roster and stores typed source identity/status in canonical order. Unknown, duplicate, or missing required sources fail closed.
6. **Private role-specific episode construction.** Remove public `EvidenceEpisodeParts`. The strategy constructs the episode through private role-specific stable identity wrappers and typed `Venue`/`InstrumentId` inputs. Numeric/timestamp/slug/window/diagnostic/config/retry fields are absent from the constructor.
7. **Incomplete identity and key contract.** Missing market/source/outcome identity does not create a partial episode. The guard returns a typed `EvidenceEpisodeRejection`; the same guard remembers each finite rejection so identical invalid input is reported once and never appended. Invalid semantic projection is likewise fail-closed and bounded to one report per stable episode without blocking a later valid key.
8. **One writer contract.** The sole `attempt_once` API accepts typed identity and semantic-key results, then returns `Appended`, `PreviouslyAttempted`, `AttemptFailedAndRetained`, typed identity rejection, or bounded semantic-key rejection. Both eligible producers use it. Submit-linked snapshots do not implement the sealed eligibility trait.
9. **Rust-only generation.** Delete both PR-local Python verifier files. A Rust generator validates TOML, renders the committed generated file deterministically, and a Rust test compares a fresh render byte-for-byte.
10. **Dimension inventory and proofs.** Record every retired key field as retained or explicitly excluded, and test every retained dimension plus volatile-field invariance, collection permutation/duplicates, payload injectivity, A-to-B-to-A, writer failure, identity rejection, and more than 4,096 episodes. The generator emits checked per-producer cardinality formulas from TOML leaf-domain sizes and source-roster rules.
11. **Memory/restart disclosure.** Per-episode state is finite; every inserted key corresponds to one attempted record. The RV snapshot carries the TOML-registered source roster separately from diagnostics so missing, duplicate, and unknown rows fail closed. Total episode memory remains monotone and unbounded across process lifetime because eviction is forbidden. Restart re-emission is accepted by this slice; durable exact-once and retirement remain #1385.

### Accepted with modification

1. **Leaf IDs.** TOML lists every leaf variant, but numeric IDs identify semantic dimensions rather than complete cross-product states or every payload combination. This keeps IDs within the frozen allocation while TOML remains the closed-domain authority. Generated exhaustive bindings reject missing/nonexistent variants.
2. **Source identity.** Runtime-configured source IDs use a private validated `RegisteredRvSourceId` wrapper. The key never contains an unvalidated/free string; the configured roster makes the domain finite for the process.
3. **Historical `Unclassified`.** Preserve deserialization compatibility in the evidence schema, but exclude `Unclassified` from the generated runtime domain. Generated conversion rejects it; there is no generic runtime state.
4. **Observability.** Keep and test `seen_episode_count`/`seen_state_count`; do not add a second runtime telemetry path in this slice. Durable capacity telemetry belongs with #1385.

### Rejected or deferred

1. **Interval timestamp in episode identity — rejected.** `same_market_interval_rollover` preserves the same market and reconstructed books; #1354 explicitly excludes timestamps and windows. A timestamp change cannot mint a new evidence episode. Tests must prove it does not reset novelty.
2. **New lifecycle/risk ordinal — deferred.** #1354 assigns risk ordinals atomically to #1385. #1470 must not invent an early ordinal or encode future capacity policy.
3. **Exit-decision/evaluation migration — deferred to #1385.** PRs #1475/#1476 explicitly record this owner boundary. #1470 must disclose that the measured exit flood remains owned by #1385, not silently migrate a recovery/risk producer.
4. **Captured exit replay in #1470 — deferred with the producer.** Covered producers get equivalent composite A-to-B-to-A wiring tests. The actual exit trace belongs to #1385.
5. **Partial/pre-selection episode variants — rejected.** They weaken stable market identity and form a fallback identity path. Identity absence is a typed, bounded rejection.
6. **String or serialized DTO keys — rejected.** Full evidence DTOs contain volatile fields and label strings. Projection uses closed typed runtime values only.

---

## Selected Dimension Inventory

| Boundary | Retained typed semantics | Explicit exclusions |
|---|---|---|
| Stable episode | strategy, configured target, execution venue, market, condition, question, ordered Up/Down instrument identities | price, timestamp, interval/window, slug, diagnostic/config fingerprint, deploy identity, retry count, capacity/risk ordinal |
| Entry skip key | registered skip reason, canonical gate blockers, canonical pricing blockers with structured payloads, fast-venue availability, reference-current-price availability, coherence, RV gate, watermark presence | prices, ages, source timestamps, source names/IDs, interval open, quantities, free text, serialized evidence DTO |
| Blocked strategy-input key | market-selection outcome, canonical gate blockers, canonical pricing blockers, selected side, fast/reference availability, reference failover, coherence, RV gate, watermark presence, canonical RV blockers, canonical TOML-roster-bound RV source states | price/notional values, source diagnostic counters/timestamps, surface/config fingerprint, provider display labels, client order ID, serialized evidence DTO |
| Recovery boundary | none; these two producers are approved non-recovery observations | every submit-linked/client-order-bearing snapshot remains a direct unsuppressed append; exit/risk/lifecycle producers remain #1385 |

Fast/reference provider labels are excluded deliberately: availability, failover, and coherence are the approved semantics; provider labels are diagnostic/config identity and must not reset novelty. RV source identity is different: the TOML-configured fixed roster is part of the canonical source-state map because status is meaningful only when bound to its registered source.

---

### Task 1: Replace the whole-state registry with a domain registry

**Files:**
- Modify: `config/evidence-novelty.toml`
- Create: `src/bolt_v3_evidence_novelty/generator.rs`
- Create: `src/bin/generate-evidence-novelty.rs`
- Modify: `src/bolt_v3_evidence_novelty/generated.rs`
- Modify: `src/bolt_v3_evidence_novelty.rs`
- Test: `tests/bolt_v3_evidence_novelty.rs`

**Interfaces:**
- Produces: `parse_registry(&str) -> Result<EvidenceNoveltyRegistry>`, `render_registry(&EvidenceNoveltyRegistry) -> Result<String>`, sealed producer markers, generated complete key structs, and generated exhaustive domain validators.
- Consumes: existing evidence enums from `bolt_v3_decision_evidence` and typed RV enums from `bolt_v3_realized_volatility`.

- [ ] **Step 1: Add RED registry tests**

Add Rust tests asserting unknown TOML keys, duplicate IDs, allocation escape, duplicate producer ownership, missing required dimensions, unknown domains, duplicate variants, and a stale generated file are rejected. Assert schema v1 whole-state rows are no longer accepted.

- [ ] **Step 2: Run the registry test target and confirm RED**

Run: `cargo test --locked --test bolt_v3_evidence_novelty -- registry --nocapture`

Expected: failures because schema-v2 parsing/rendering and generated producer keys do not exist.

- [ ] **Step 3: Define schema-v2 domains and producers**

Use this shape, with every concrete variant listed in the real file:

```toml
schema_version = 2

[[domain]]
name = "entry_skip_reason"
rust_type = "BoltV3EntrySkipReasonCategory"
variants = ["StrategyCoreNotRegistered", "EntryGateBlocked"]

[[producer]]
name = "entry_skip"
rust_marker = "EntrySkipProducer"
rust_key = "EntrySkipSemanticKey"
owner = "EntrySkip"
allocation = "strategy_input_pricing_blocker"

[[producer.dimension]]
id = 144
name = "reason"
domain = "entry_skip_reason"
shape = "scalar"
```

Register reusable domains for skip reason, gate blocker, forced-flat payload, exposure occupancy, pricing blocker, outcome side, edge blocker, RV gate, RV blocker, RV source status, RV source rejection, and availability/coherence/failover states. Register dimensions, not state combinations.

- [ ] **Step 4: Implement the Rust parser and deterministic renderer**

The parser uses `#[serde(deny_unknown_fields)]` input structs and validates exact uniqueness/allocation/domain/producer constraints. The renderer emits private key fields plus generated constructors and exhaustive validator matches without wildcard arms.

- [ ] **Step 5: Add the single Rust generator command**

`src/bin/generate_evidence_novelty.rs` reads `config/evidence-novelty.toml`, renders once through the shared generator module, and writes only `src/bolt_v3_evidence_novelty/generated.rs`. It accepts no alternate registry or output path.

- [ ] **Step 6: Generate and verify byte equality**

Run: `cargo run --locked --bin generate-evidence-novelty`

Run: `cargo test --locked --test bolt_v3_evidence_novelty -- generated_registry --nocapture`

Expected: the committed generated file equals a fresh render byte-for-byte.

### Task 2: Implement typed identity, canonical components, and the generic guard

**Files:**
- Modify: `src/bolt_v3_evidence_novelty.rs`
- Modify: `src/bolt_v3_evidence_novelty/generated.rs`
- Test: `tests/bolt_v3_evidence_novelty.rs`

**Interfaces:**
- Produces: `EvidenceEpisodeId`, `EvidenceEpisodeRejection`, `CanonicalSet<T>`, `RegisteredRvSourceId`, `CanonicalSourceStates`, `EvidenceNoveltyGuard<P>`, and `EvidenceAttemptOutcome`.

- [ ] **Step 1: Add RED guard and identity tests**

Cover private stable identity construction, wrong venue, duplicate/reversed outcomes, incomplete identity rejection, typed source-roster rejection, duplicate canonical components, permutation equality, swapped payload inequality, A-to-B-to-A fixed cardinality, and 4,097-episode revisit.

- [ ] **Step 2: Replace public generic string parts**

Expose one crate-private binary-market constructor whose parameters are role-specific wrappers plus `Venue` and ordered `(OutcomeSide, InstrumentId)` values. Do not expose `From<String>` or numeric/time parameters.

- [ ] **Step 3: Implement canonical wrappers**

```rust
pub struct CanonicalSet<T>(BTreeSet<T>);

impl<T: Ord> CanonicalSet<T> {
    pub fn try_from_iter(values: impl IntoIterator<Item = T>) -> Result<Self>;
}
```

Reject a duplicate when insertion returns `false`. Validate every RV source against the configured roster before constructing `CanonicalSourceStates`.

- [ ] **Step 4: Implement the sealed generic guard**

```rust
pub struct EvidenceNoveltyGuard<P: NoveltyEligibleProducer> {
    seen_by_episode: BTreeMap<EvidenceEpisodeId, BTreeSet<P::Key>>,
    rejected_identities: BTreeSet<EvidenceEpisodeRejection>,
    marker: PhantomData<P>,
}

pub enum EvidenceAttemptOutcome {
    Appended,
    PreviouslyAttempted,
    AttemptFailedAndRetained(anyhow::Error),
    IdentityRejectedFirst(EvidenceEpisodeRejection),
    IdentityRejectedPreviously(EvidenceEpisodeRejection),
}
```

`attempt_once` inserts before calling the writer. It never describes a failed write as appended and never retries an already-attempted key.

- [ ] **Step 5: Run guard tests**

Run: `cargo test --locked --test bolt_v3_evidence_novelty -- --nocapture`

Expected: all identity, canonicalization, generated-registry, and guard behavior tests pass.

### Task 3: Build complete typed producer projections

**Files:**
- Modify: `src/bolt_v3_decision_evidence.rs`
- Modify: `src/bolt_v3_realized_volatility.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/entry_decision.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`

**Interfaces:**
- Produces: typed entry-skip and blocked-RV semantic inputs consumed by generated `EntrySkipSemanticKey::try_new` and `BlockedStrategyInputSemanticKey::try_new`.

- [ ] **Step 1: Preserve the three existing RED assertions**

Do not edit their expected count of two. Add tests for gate blockers, fast/reference/incoherence liveness, RV gate, watermark, RV blockers, normalized source status, swapped pricing payloads, canonical permutations, duplicate rejection, volatile price/time invariance, and composite A-to-B-to-A.

- [ ] **Step 2: Make key-domain evidence enums orderable**

Add `PartialOrd, Ord` only to closed enums used in generated keys and their payload enums. Preserve serialization names and evidence payloads.

- [ ] **Step 3: Carry typed RV semantic state beside durable labels**

Extend the internal `RealizedVolatilityEvidenceFields` with typed blocker and typed normalized source-state data derived directly from `RealizedVolSnapshot`. Keep the existing string fields unchanged for durable evidence serialization; keys never read those strings.

- [ ] **Step 4: Remove the string reason seam from runtime construction**

Change internal `EntrySubmissionDecision.blocked_reason` and `EntryEvaluationLogFields.submission_blocked_reason` to the typed category. Preserve legacy deserialization only in `bolt_v3_decision_evidence`; generated runtime conversion rejects `Unclassified`.

- [ ] **Step 5: Construct generated complete keys once**

Entry projection supplies all approved entry dimensions. Blocked-RV projection supplies all approved blocked dimensions and validates configured RV source identity. There is no blocker-to-ID match in `mod.rs`.

- [ ] **Step 6: Run affected source-evidence tests**

Run the three named failing tests plus every newly added semantic-dimension test. Expected: every distinct complete key emits once, identical/volatile-only repeats do not, and A-to-B-to-A emits exactly two records.

### Task 4: Route both producers through one attempt contract

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Test: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`

**Interfaces:**
- Consumes: generated producer markers and complete keys from Tasks 1–3.
- Produces: uniform at-most-one-attempt behavior for entry skip and blocked-RV only.

- [ ] **Step 1: Add RED writer and identity failure tests**

For both producers, prove a failed writer is called once, produces zero durable records, and the identical next tick is suppressed. Prove incomplete identity appends nothing and reports each typed rejection once. Prove submit-linked snapshots append repeatedly with evolving payloads.

- [ ] **Step 2: Replace manual `has_claimed`/`claim_once` sequences**

Both producer methods build evidence and invoke `attempt_once`. Match every `EvidenceAttemptOutcome` explicitly; failure is logged once and retained, while eligibility/construction errors fail closed.

- [ ] **Step 3: Prove interval timestamp changes do not reset identity**

Apply `same_market_interval_rollover` with unchanged market/source/outcomes, replay the same semantic state, and assert no second record. A genuinely different stable market identity must emit once.

- [ ] **Step 4: Run the complete strategy test module remotely or through the focused Rust evidence workflow**

Expected: the affected strategy tests pass without changing recovery-bearing paths.

### Task 5: Remove the retired Python/source-scanning lane

**Files:**
- Delete: `scripts/verify_bolt_v3_evidence_novelty.py`
- Delete: `scripts/test_verify_bolt_v3_evidence_novelty.py`
- Modify: `Cargo.toml` only if the generator binary requires an explicit declaration
- Modify: governed source-fence/justfile references only where they point at these deleted files

**Interfaces:**
- Produces: one Rust parser/renderer/test lane; no Python imports or Rust-source regex checks.

- [ ] **Step 1: Delete both PR-local Python files**

- [ ] **Step 2: Replace only their legitimate checks**

TOML structural validation, deterministic rendering, byte comparison, and exhaustive enum binding live in Rust. Do not port source scans.

- [ ] **Step 3: Prove the lane is gone**

Run: `rg -n "rust_source_scanner|lane_governor|verify_bolt_v3_evidence_novelty|EvidenceCanonicalState" scripts src tests justfile .github/workflows`

Expected: no novelty-lane Python/source-parser dependency and no whole-state enum runtime dispatch.

### Task 6: Verification, disclosure, and review handoff

**Files:**
- Modify: PR #1470 body/comment through GitHub after the exact head exists
- Modify: this plan only if implementation evidence changes an adjudicated decision

- [ ] **Step 1: Run non-compile preflight**

Discover the current governed commands from `justfile` and `.github/workflows`; run formatting/static/source-fence checks without restoring the removed Python lane.

- [ ] **Step 2: Self-review against every accepted finding**

Inspect the diff for scope creep, fallback branches, string semantic keys, recovery-bearing suppression, collection non-canonicality, writer contract divergence, interval-time leakage, and generated/TOML drift.

- [ ] **Step 3: Commit and governed push**

Commit only #1470 scope, use the repository's governed push path, and record the exact head SHA.

- [ ] **Step 4: Obtain exact-head remote Rust evidence**

Dispatch the smallest affected-test workflow, not Final Review. Record command, test counts, run URL, and exact SHA. Stop after at most two diagnostic Rust Probe runs.

- [ ] **Step 5: Conduct internal adversarial review**

Review the exact pushed diff against the declared PR scope and every external finding. Resolve every substantive finding and every unresolved thread before requesting any external review.

- [ ] **Step 6: Publish the evidence comment**

Post exact SHA, commands, counts, workflow links, root cause, selected design, external recommendation adjudication, writer/recovery boundary, #1385 exit deferral, and worktree cleanliness to PR #1470.

- [ ] **Step 7: Prepare but do not send the final external PR-review request**

The prompt must ask for code review at the exact head, include evidence links/counts, unresolved risks, and the accepted design contract. Do not request code-owner review and do not merge.

---

## Self-Review

- **Spec coverage:** The plan maps stable identity, complete semantic transitions, identical-tick suppression, A-to-B-to-A, unknown fail-closed behavior, bounded per-episode state, no eviction, deterministic TOML authority, recovery exclusions, non-compile preflight, exact-head remote evidence, adversarial review, PR comment, and external prompt to explicit tasks.
- **Scope boundary:** Exit/risk novelty, dependent PRs, backtesting, restart exact-once, retirement, and capacity policy remain outside #1470.
- **Type consistency:** Generated producer markers supply complete key types to `EvidenceNoveltyGuard<P>`; strategy projections construct those keys from typed evidence; `attempt_once` is the sole writer-facing novelty operation.
- **Placeholder scan:** The plan contains no deferred implementation placeholders; each deferred scope names its owning issue.
