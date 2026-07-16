# Issue 1354 Canonical Evidence Novelty Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace arrival-ordered evidence dedupe with fixed canonical IDs, monotonic per-episode novelty, and a complete stable market identity for the two #1354 tracer-bullet producers.

**Architecture:** TOML owns named canonical states inside the frozen market-family allocations. Generated Rust supplies typed IDs, and a per-episode fixed bitset records claims without eviction. Market-family selection supplies a complete evidence identity before strategy runtime state is built.

**Tech Stack:** Rust, TOML, Python 3 `tomllib`, repository `just` verification recipes, GitHub Actions Rust Probe.

## Global Constraints

- Preserve the frozen market allocations: strategy-input/pricing-blocker `144..208`; terminal/closed-window-skip `240..256` remains untouched.
- Preserve the direct submit-linked `strategy_input_snapshot` append path.
- Do not add persistence, retirement, recovery-reader, trading, deploy, or live-operation behavior.
- Runtime values and canonical numeric IDs come from TOML-generated Rust.
- Rust compilation and tests run remotely; local verification is formatting, Python/static, workflow, dependency, and source-fence only.
- Every Rust Probe uses a clean pushed head; at most two probes run before full exact-head verification.

---

### Task 1: Make TOML the canonical ID authority

**Files:**
- Modify: `config/evidence-novelty.toml`
- Modify: `scripts/verify_bolt_v3_evidence_novelty.py`
- Modify: `scripts/test_verify_bolt_v3_evidence_novelty.py`
- Regenerate: `src/bolt_v3_evidence_novelty_generated.rs`

**Interfaces:**
- Consumes: frozen market allocations and the existing deterministic generator.
- Produces: `EvidenceCanonicalState`, `EvidenceStateRegistration`, `canonical_state_registration`, and `registered_evidence_state_by_id`.

- [ ] **Step 1: Add failing verifier tests**

Add tests that require seven exact market allocation rows, reject state IDs outside their allocation, reject duplicate IDs/names, preserve unassigned IDs, and render these permanent mappings:

```text
144..155  BlockedStrategyInput{RvGateResult}{WatermarkAbsent|WatermarkPresent}
156..172  EntrySkip{each non-Unclassified BoltV3EntrySkipReasonCategory in declaration order}
```

- [ ] **Step 2: Verify RED locally**

Run: `python3 scripts/test_verify_bolt_v3_evidence_novelty.py`

Expected: failures because the current schema has two capacity ranges and no individual canonical IDs or frozen allocations.

- [ ] **Step 3: Replace capacity rows with allocation and state rows**

Use this closed schema (the example is one of 29 state rows):

```toml
schema_version = 1

[family]
name = "market"
capacity = 256

[[allocation]]
name = "strategy_input_pricing_blocker"
id_start = 144
id_end_exclusive = 208

[[state]]
rust_variant = "BlockedStrategyInputRejectedNotReadyWatermarkPresent"
producer_kind = "strategy_input_snapshot"
semantic_state = "strategy_input_snapshot.blocked_rv.rejected_not_ready.watermark_present"
allocation = "strategy_input_pricing_blocker"
id = 155
```

Include all seven frozen allocation rows and all 29 named state rows. Validate exact keys, allocation boundaries, unique variants, unique producer/state pairs, unique IDs, and that every state ID belongs to its named allocation. Do not require unassigned IDs to be populated.

- [ ] **Step 4: Generate typed permanent IDs**

Render:

```rust
#[repr(u16)]
pub enum EvidenceCanonicalState {
    BlockedStrategyInputAcceptedWatermarkAbsent = 144,
    BlockedStrategyInputAcceptedWatermarkPresent = 145,
    BlockedStrategyInputMissingSnapshotWatermarkAbsent = 146,
    BlockedStrategyInputMissingSnapshotWatermarkPresent = 147,
    BlockedStrategyInputMissingEvaluationEventTimeWatermarkAbsent = 148,
    BlockedStrategyInputMissingEvaluationEventTimeWatermarkPresent = 149,
    BlockedStrategyInputRejectedFutureDatedWatermarkAbsent = 150,
    BlockedStrategyInputRejectedFutureDatedWatermarkPresent = 151,
    BlockedStrategyInputRejectedStaleWatermarkAbsent = 152,
    BlockedStrategyInputRejectedStaleWatermarkPresent = 153,
    BlockedStrategyInputRejectedNotReadyWatermarkAbsent = 154,
    BlockedStrategyInputRejectedNotReadyWatermarkPresent = 155,
    EntrySkipStrategyCoreNotRegistered = 156,
    EntrySkipEntryGateBlocked = 157,
    EntrySkipEntryPricingBlocked = 158,
    EntrySkipNoSideSelected = 159,
    EntrySkipSizedNotionalNotPositive = 160,
    EntrySkipInstrumentIdMissing = 161,
    EntrySkipInstrumentMissingFromCache = 162,
    EntrySkipEntryPriceMissing = 163,
    EntrySkipQuantityRoundingFailed = 164,
    EntrySkipLimitNotionalExceedsSizedNotional = 165,
    EntrySkipEntryQuoteNotionalBelowVenueMinimum = 166,
    EntrySkipEntryQuoteNotionalMinimumUnmodeled = 167,
    EntrySkipQuantityNotPositive = 168,
    EntrySkipPositionContractInvalid = 169,
    EntrySkipEntryPositionContractUnsupported = 170,
    EntrySkipHistoricalEntryFeeUnavailable = 171,
    EntrySkipOnePositionInvariantViolation = 172,
}

pub struct EvidenceStateRegistration {
    pub state: EvidenceCanonicalState,
    pub owner: EvidenceStateOwner,
    pub producer_kind: &'static str,
    pub semantic_state: &'static str,
    pub id: usize,
}
```

Generate exhaustive lookup functions by enum and numeric ID. Unknown numeric IDs return `Err` from the handwritten wrapper.

- [ ] **Step 5: Verify GREEN and generated-byte determinism**

Run:

```bash
python3 scripts/verify_bolt_v3_evidence_novelty.py --write
python3 scripts/test_verify_bolt_v3_evidence_novelty.py
python3 scripts/verify_bolt_v3_evidence_novelty.py
```

Expected: all commands pass and a second `--write` produces no diff.

- [ ] **Step 6: Commit the registry slice**

```bash
git add config/evidence-novelty.toml scripts/verify_bolt_v3_evidence_novelty.py scripts/test_verify_bolt_v3_evidence_novelty.py src/bolt_v3_evidence_novelty_generated.rs
git commit -m "fix(evidence): assign canonical novelty ids"
```

### Task 2: Specify the Rust contract before implementation

**Files:**
- Modify: `tests/bolt_v3_evidence_novelty.rs`
- Modify: `tests/wiring_registration.rs`

**Interfaces:**
- Consumes: generated `EvidenceCanonicalState` and current public novelty API.
- Produces: executable regressions for fixed IDs, unknown IDs, episode A-to-B-to-A, and complete identity components.

- [ ] **Step 1: Replace generic-state tests with canonical-state tests**

Add tests with these assertions:

```rust
assert_eq!(EvidenceCanonicalState::EntrySkipStrategyCoreNotRegistered as usize, 156);
assert_eq!(EvidenceCanonicalState::EntrySkipOnePositionInvariantViolation as usize, 172);
assert!(registered_evidence_state_by_id(143).is_err());
assert!(registered_evidence_state_by_id(173).is_err());
```

Exercise duplicate claims using actual entry-skip and blocked-snapshot canonical variants.

- [ ] **Step 2: Add the episode-churn regression**

Claim one canonical state for episode A, then B, then A again. Assert only two payloads/appends occur and both episode domains remain present. Repeat across 4,097 distinct intervening episode IDs and assert returning to the first episode remains suppressed.

- [ ] **Step 3: Add complete identity tests**

Construct `EvidenceEpisodeParts` with `negative_risk` and exactly two `EvidenceOutcomeIdentity` values. Independently change strategy, target, venue, Gamma market, condition, question, negative-risk mode, outcome index, normalized label, and CLOB token ID; every change must change the ID. Reject duplicate indices, labels, or token IDs.

- [ ] **Step 4: Commit and publish the RED head**

```bash
git add tests/bolt_v3_evidence_novelty.rs tests/wiring_registration.rs
git commit -m "test(evidence): specify canonical episode novelty"
just sandbox-safe-push
just rust-probe suggest
just rust-probe nextest-test-target bolt_v3_evidence_novelty
```

Expected Rust Probe result: compilation/test failure because the new identity and guard APIs are not implemented. This is Rust Probe run 1 of 2.

### Task 3: Implement complete stable market identity

**Files:**
- Modify: `src/bolt_v3_market_families/mod.rs`
- Modify: `src/bolt_v3_market_families/updown.rs`
- Modify: `src/bolt_v3_market_families/static_binary_event.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/selection.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/runtime_state.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs`

**Interfaces:**
- Consumes: NT `BinaryOption::raw_symbol`, `BinaryOption::outcome`, and `info["neg_risk"]` at the family-owned metadata seam.
- Produces: `SelectedMarketEvidenceIdentity`, carried unchanged from selection into `EvidenceEpisodeId`.

- [ ] **Step 1: Add typed upstream identity**

Define in `bolt_v3_market_families/mod.rs`:

```rust
pub struct SelectedMarketEvidenceOutcome {
    pub index: u8,
    pub normalized_outcome: String,
    pub clob_token_id: String,
}

pub struct SelectedMarketEvidenceIdentity {
    pub gamma_market_id: String,
    pub condition_id: String,
    pub question_id: String,
    pub negative_risk: bool,
    pub outcomes: [SelectedMarketEvidenceOutcome; 2],
}
```

Add `evidence_identity` to `SelectedBinaryOptionMarket`, `CandidateMarket`, and `ActiveMarketState`.

- [ ] **Step 2: Bind identity at each supported family seam**

For up/down and static binary families, read `market_id`, `condition_id`, `question_id`, `neg_risk`, exact `raw_symbol()` token ID, and normalized strategy side while inspecting `InstrumentAny::BinaryOption`. Require both legs to agree on market fields and negative-risk mode. Order outcomes by canonical side with indices `0` and `1`; reject duplicate normalized labels or token IDs by returning no candidate.

- [ ] **Step 3: Construct episodes only from the binding**

Replace prepared-book extraction in `BinaryOracleEdgeTaker::evidence_episode_id` with conversion from `active.evidence_identity`. Define `EvidenceOutcomeIdentity` and make `EvidenceEpisodeParts.outcomes` an exact two-element array. Validate non-empty values, distinct indices, labels, and tokens.

- [ ] **Step 4: Update fixtures without adding fallback identity paths**

All selected-market and candidate-market fixtures construct the same complete binding. Do not parse token IDs back out of `InstrumentId` inside the strategy.

- [ ] **Step 5: Commit the identity slice**

```bash
git add src/bolt_v3_market_families/mod.rs src/bolt_v3_market_families/updown.rs src/bolt_v3_market_families/static_binary_event.rs src/strategies/binary_oracle_edge_taker/selection.rs src/strategies/binary_oracle_edge_taker/runtime_state.rs src/strategies/binary_oracle_edge_taker/mod.rs src/strategies/binary_oracle_edge_taker/tests/shared_fixture.rs src/bolt_v3_evidence_novelty.rs
git commit -m "fix(evidence): bind complete episode identity"
```

### Task 4: Implement fixed-bit canonical novelty and producer mappings

**Files:**
- Modify: `src/bolt_v3_evidence_novelty.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/entry_decision.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/mod.rs`
- Modify: `src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs`
- Modify: `scripts/verify_bolt_v3_evidence_novelty.py`

**Interfaces:**
- Consumes: typed canonical states and complete `EvidenceEpisodeId` from Tasks 1 and 3.
- Produces: `EvidenceNoveltyGuard::claim_once(&EvidenceEpisodeId, EvidenceCanonicalState)` with no eviction.

- [ ] **Step 1: Replace the generic guard**

Use:

```rust
pub struct EvidenceNoveltyGuard {
    owner: EvidenceStateOwner,
    seen_by_episode: BTreeMap<EvidenceEpisodeId, Vec<u64>>,
}
```

Size each bitset from the TOML-generated family capacity. Before setting a bit, resolve the state registration and require its owner to match the guard. Duplicate bits return `Ok(false)`; unknown IDs cannot be constructed and numeric lookup rejects them.

- [ ] **Step 2: Map entry skips exhaustively**

Add an exhaustive conversion from every non-`Unclassified` `BoltV3EntrySkipReasonCategory` to its generated `EntrySkip*` canonical state. `Unclassified` remains rejected before payload construction. Remove `EntrySkipSemanticState` and its arrival-ordered compound key.

- [ ] **Step 3: Map blocked snapshots exhaustively**

Map the six `BoltV3RvGateResult` variants and watermark boolean to the twelve generated blocked-snapshot canonical states. Keep source diagnostics, blockers, selection outcome, and availability in the payload only. Remove `BlockedStrategyInputSemanticState` and its unrestricted string identity.

- [ ] **Step 4: Strengthen the static verifier**

Require each producer claim to use a generated canonical mapping, require the claim to precede payload construction/append, and keep the direct submit-linked snapshot assertion. Reject references to removed semantic-state structs.

- [ ] **Step 5: Publish and verify GREEN with the second probe**

```bash
git add src/bolt_v3_evidence_novelty.rs src/strategies/binary_oracle_edge_taker/entry_decision.rs src/strategies/binary_oracle_edge_taker/mod.rs src/strategies/binary_oracle_edge_taker/tests/source_evidence.rs scripts/verify_bolt_v3_evidence_novelty.py tests/bolt_v3_evidence_novelty.rs tests/wiring_registration.rs
git commit -m "fix(evidence): retain canonical novelty per episode"
just sandbox-safe-push
just rust-probe suggest
just rust-probe nextest-test-target bolt_v3_evidence_novelty
```

Expected: Rust Probe passes. This is Rust Probe run 2 of 2.

### Task 5: Verify the coherent exact head

**Files:**
- Modify only files required by concrete verification failures.

**Interfaces:**
- Consumes: completed canonical registry, identity, guard, and producer mappings.
- Produces: clean exact-head evidence suitable for renewed review.

- [ ] **Step 1: Run allowed local checks**

```bash
just fmt-check
python3 scripts/test_verify_bolt_v3_evidence_novelty.py
python3 scripts/verify_bolt_v3_evidence_novelty.py
just deny
just ci-lint-workflow
just source-fence-static
git diff --check 66836cd38ee8bd1931a8a068c885fa481d2efe03..HEAD
```

Expected: every command succeeds.

- [ ] **Step 2: Commit only evidence-backed corrections**

If a check exposes a concrete defect, add its regression first, fix that defect, rerun the failed check, and commit the exact files with a scoped `fix(evidence): ...` message. Do not change unrelated code.

- [ ] **Step 3: Publish the coherent draft head and run full remote verification**

```bash
just sandbox-safe-push
just verify-remote
```

Expected: exact remote head equals local `HEAD`; full remote verification succeeds for that SHA.

- [ ] **Step 4: Perform final review hygiene**

Confirm the worktree is clean, generated bytes are deterministic, the PR remains draft, submit-linked snapshots are unchanged, all review findings are resolved in code/tests, and the exact-head evidence is recorded outside the stable PR body. Do not merge, deploy, or trade.
