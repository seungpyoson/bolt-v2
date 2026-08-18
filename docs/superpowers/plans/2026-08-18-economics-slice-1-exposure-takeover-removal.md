# Economics Slice 1 Exposure Takeover Removal Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Do not delegate this plan.

**Goal:** Remove the strategy-bound exposure takeover from PR #1544 while preserving the approved economics Slice 1, maker, OMS-validation, and fee-aware exit changes.

**Architecture:** Restore the edge-taker exposure boundary from `623801311` and replay only the later changes that belong to economics, maker routing, OMS validation, and forced-reduction deletion. Keep the shared route-attempt participant because maker resting registration uses it, but remove exposure-only configuration, evidence, position-episode plumbing, and strategy callbacks. This is an additive cleanup commit; branch history is not rewritten.

**Tech Stack:** Rust, Cargo, TOML, Git

**Spec:** `docs/superpowers/specs/2026-08-09-economics-slice-1-current-main-design.md`

## Global Constraints

- PR #1544 remains scoped to issue #1445 economics Slice 1 plus its already-reviewed maker and OMS boundary repairs.
- Strategies produce intent only; no strategy-owned submit, cancellation, reconciliation, or replacement lifecycle authority is introduced.
- `BoltV3RouteAttemptParticipant` remains shared because maker resting registration consumes it.
- No NautilusTrader pin change or upstream reconciliation workaround is included.
- No source-scanning test is added; verification uses behavior tests, compilation, formatting, Clippy, and the production conditional census.
- The cleanup must not enable live trading; existing fail-closed reconciliation and admission controls remain unchanged.

---

### Task 1: Lock the strategy configuration boundary

**Files:**
- Modify: `src/strategies/binary_oracle_edge_taker/tests/config.rs`

**Interfaces:**
- Consumes: `valid_raw_config()` and `BinaryOracleEdgeTakerBuilder::parse_config`.
- Produces: a behavior regression proving the strategy no longer requires exposure-authority retention limits.

- [x] **Step 1: Add the failing behavior test**

```rust
#[test]
fn runtime_config_does_not_require_exposure_authority_limits() {
    let mut raw = valid_raw_config();
    raw.as_table_mut()
        .expect("valid config must be a table")
        .remove("exposure_obligations");

    BinaryOracleEdgeTakerBuilder::parse_config(&raw)
        .expect("strategy config must not own shared exposure-authority limits");
}
```

- [x] **Step 2: Verify the test fails for the current takeover**

Run:

```bash
cargo test --locked --lib runtime_config_does_not_require_exposure_authority_limits -- --nocapture
```

Expected: failure naming the missing `exposure_obligations` field.

---

### Task 2: Restore the compact edge-taker boundary

**Files:**
- Restore from `623801311`: `src/strategies/binary_oracle_edge_taker/{archetype.rs,config.rs,entry_decision.rs,exposure.rs,mod.rs}` and exposure-takeover test changes.
- Preserve/replay: fee-adjusted exit comparison from `589f5241`, OMS fixture changes from `2cc1e3a5`, and forced-reduction deletion from `e01083eb`.
- Modify as compilation requires: `src/strategies/binary_oracle_edge_taker/{exit_decision.rs,mod.rs,tests/*.rs}`.

**Interfaces:**
- Consumes: the pre-takeover strategy boundary at `623801311` and the current shared economics/admission APIs.
- Produces: the compact pre-takeover exposure state integrated with current economics, OMS validation, and evidence APIs.

- [x] **Step 1: Mechanically restore the pre-takeover strategy files**

Use the exact `623801311` tree as the deletion authority. Do not restore `tests/config.rs`, which contains Task 1's regression.

- [x] **Step 2: Replay the valid economics behavior**

Preserve the `589f5241` behavior that compares exit and hold using final sealed, fee-adjusted economics. Do not restore scalar fee math or a strategy-owned economics provider.

- [x] **Step 3: Replay OMS and admission deletions**

Preserve the `2cc1e3a5` test-fixture OMS updates and the `e01083eb` removal of forced-reduction routing. Do not reintroduce unsupported OMS modes or `KillSwitchForcedReduction`.

- [x] **Step 4: Compile the binary**

Run:

```bash
cargo check --locked --bin bolt-v2
```

Expected: the known exposure-only shared seams remain visible as compiler errors. Record the exact errors, then remove those seams in Task 3; do not treat this intermediate checkpoint as a completion gate.

---

### Task 3: Remove shared exposure-only seams

**Files:**
- Modify: `src/bolt_v3_config.rs`
- Modify: `src/bolt_v3_order_execution.rs`
- Modify: `src/bolt_v3_current_evidence/{facts.rs,codec.rs,codec/entry_skip.rs,codec/exit.rs,codec/lifecycle.rs,codec/settlement.rs,generated_contract.rs}`
- Modify: `config/{decision-evidence-contract.toml,evidence-novelty.toml}`
- Modify: `config/strategies/binary_oracle_{bnb,btc,doge,eth,sol,xrp}.toml`
- Modify: `tests/fixtures/bolt_v3/strategies/binary_oracle.toml`

**Interfaces:**
- Consumes: current maker use of `BoltV3RouteAttemptParticipant` and current exit-order authority API.
- Produces: shared execution without strategy-specific position fingerprints, obligation limits, or exposure-only evidence variants.

- [x] **Step 1: Delete exposure-only configuration**

Remove `ExposureObligationLimits` and every `[parameters.runtime.exposure_obligations]` block. Keep all economics and OMS configuration unchanged.

- [x] **Step 2: Collapse exit authority back to its pre-takeover identity**

Remove `BoltV3PositionEpisodeFingerprint` and copy-on-write episode rebasing introduced solely for the strategy reducer. Keep the generic route-attempt participant and maker registration participant intact.

- [x] **Step 3: Delete exposure-only evidence states**

Remove the operation-generation entry/exit reasons and takeover lifecycle outcomes/transitions. Restore the three affected evidence schema versions to their pre-takeover values, then regenerate `generated_contract.rs`:

```bash
cargo run --locked --bin generate_decision_evidence_contract
```

- [x] **Step 4: Re-run the Task 1 regression and focused strategy tests**

```bash
cargo test --locked --lib runtime_config_does_not_require_exposure_authority_limits -- --nocapture
cargo test --locked --features test-current-evidence-inspection --lib binary_oracle_edge_taker::tests -- --test-threads=1
```

Expected: both commands pass.

---

### Task 4: Remove superseded takeover documentation

**Files:**
- Delete: `docs/superpowers/specs/2026-08-18-shared-exposure-authority-replacement-design.md`
- Modify or delete if wholly takeover-specific: `docs/superpowers/specs/2026-08-18-economics-slice-1-external-review-repairs-design.md`
- Modify or delete if wholly takeover-specific: `docs/superpowers/plans/2026-08-18-economics-slice-1-external-review-repairs.md`
- Modify: `docs/superpowers/{specs,plans}/2026-08-10-economics-slice-1-review-repairs*.md`
- Modify: PR #1544 body after the code is pushed.

**Interfaces:**
- Consumes: the approved #1445 design and final cleaned diff.
- Produces: lasting documentation that does not claim strategy-bound exposure ownership or hide deferred upstream reconciliation work.

- [x] **Step 1: Remove the unpushed replacement design**

Delete the rejected shared-exposure replacement design instead of implementing or publishing it.

- [x] **Step 2: Correct lasting scope records**

Retain historical economics/maker repair rationale, but remove statements claiming the takeover is current architecture. State that shared exposure recovery is separate future work blocked on an NT-native reconciliation/durability contract.

- [x] **Step 3: Check documentation mechanically**

```bash
rg -n "GovernedExposure|ExposureObligationLimits|shared exposure authority replacement" docs/superpowers
git diff --check
```

Expected: no current-plan or current-design claim requires the removed takeover; historical references are explicitly marked superseded if retained.

---

### Task 5: Verify and publish the cleanup

**Files:**
- Modify: this plan's checkboxes with exact evidence.
- External: PR #1544 body and review requests only after a clean pushed head.

**Interfaces:**
- Consumes: the final cleanup diff.
- Produces: exact-head local evidence, a pushed commit, corrected PR scope, and fresh independent review requests.

- [x] **Step 1: Run formatting and diff checks**

```bash
cargo fmt --all --check
git diff --check
```

- [x] **Step 2: Run focused compile and lint checks**

```bash
cargo check --locked --bin bolt-v2
cargo clippy --locked --bin bolt-v2 -- -D warnings
```

- [x] **Step 3: Run the focused behavior suite**

```bash
cargo test --locked --features test-current-evidence-inspection --lib binary_oracle_edge_taker::tests -- --test-threads=1
```

Evidence: edge-taker `429/429`, shared order execution `50/50`, and maker/taker integration `73/73` passed.

- [x] **Step 4: Recompute the production conditional census**

Use the same lexical production-Rust scanner and the exact range `623801311..HEAD`. Record `if`, `match`, and combined net counts; do not reuse the projected reduction.

Evidence: production Rust moved from `if=4604, match=1612, combined=6216` at `623801311` to `if=4627, match=1737, combined=6364` in the final worktree: net `if=+23`, `match=+125`, combined `+148`. The rejected takeover head was combined `+424`, so this cleanup removes 276 conditional-bearing production lines; the edge-taker module contributes only `+3` at the final worktree.

- [x] **Step 5: Commit and push**

```bash
git add <the reviewed cleanup paths>
git commit -m "refactor(exposure): remove strategy-bound takeover"
git push
```

- [ ] **Step 6: Update PR scope and request fresh reviews**

Update the stable PR body without transient head/check status, resolve applicable review threads, and request review from the required code owner plus the external models. Do not merge.
