# Redemption Review Findings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace predictive Rust-source fencing with one compiler-enforced, one-use capability while retaining the zeroizing signer fix and deterministic configuration evidence.

**Architecture:** `prepare_redemption_request` consumes an opaque permit that safe production code cannot construct. The Python verifier is reduced to parsed TOML, exact pins, hashes, manifest wiring, and generator drift; Rust compile-fail and behavior tests own Rust-language evidence.

**Tech Stack:** Rust, Python 3.11 `unittest`/`tomllib`, repository `just` verification recipes.

## Global Constraints

- No regex or Rust-source prediction.
- No redemption activation, submission, durable authority, or production caller.
- No fallback entrypoint, compatibility path, caller allowlist, or runtime branch for permit issuance.
- Preserve request bytes, signing, retry, lease, callback, and identity behavior.
- Do not run local compile-heavy Rust verification; use exact-head remote Rust evidence.

---

### Task 1: Remove Predictive Source Verification

**Files:**
- Modify: `scripts/test_verify_polymarket_redemption_preparation.py`
- Modify: `scripts/verify_polymarket_redemption_preparation.py`

**Interfaces:**
- Consumes: parsed `config/*.toml`, parsed `Cargo.toml`, pinned evidence constants, generated projection.
- Produces: `boundary_errors(root: pathlib.Path) -> list[str]` without interpreting Rust source.

- [ ] **Step 1: Add the failing no-prediction regression**

Replace caller-scanner mutation tests with a test that writes arbitrary caller-like Rust text outside the required artifacts and expects `boundary_errors` to remain empty. The current scanner must fail this test with an `active production caller` error.

- [ ] **Step 2: Run the focused regression and verify RED**

Run:

```bash
python3 scripts/test_verify_polymarket_redemption_preparation.py \
  PolymarketRedemptionPreparationVerifierTests.test_source_text_is_not_policy_input
```

Expected: FAIL because the current verifier interprets Rust source.

- [ ] **Step 3: Replace source inference with parsed-data checks**

Delete the Rust lexer, Rust source discovery, function extraction, marker ordering, macro sink enumeration, and caller scanning. Remove `re` entirely. Parse `Cargo.toml` with `tomllib`; compare exact dependency values and the `polymarket_redemption_preparation` test target. Walk parsed TOML mappings to enforce one authority for lifecycle keys and compare evidence dictionaries and deployment hash exactly.

- [ ] **Step 4: Run focused and full Python verification and verify GREEN**

Run:

```bash
python3 scripts/test_verify_polymarket_redemption_preparation.py
python3 scripts/verify_polymarket_redemption_preparation.py
```

Expected: all tests pass and repository verification exits zero.

### Task 2: Add the One-Use Compiler Capability

**Files:**
- Modify: `tests/polymarket_redemption_preparation_compile_fail.rs`
- Modify: `src/bolt_v3_polymarket_redemption.rs`

**Interfaces:**
- Produces: `pub struct RedemptionPreparationPermit { private: () }` with no production constructor.
- Changes: `prepare_redemption_request(permit: RedemptionPreparationPermit, lease: &mut RiskClosureWorkspaceLease, ...)` consumes the permit by value.

- [ ] **Step 1: Add compile-fail capability cases before implementation**

Add cases that external code cannot construct `RedemptionPreparationPermit { private: () }` and cannot call `.clone()` on a received permit. Update existing compile-fail snippets to receive a permit parameter so they continue proving lease, callback-lifetime, and serialization boundaries independently.

- [ ] **Step 2: Record the deferred RED expectation**

Do not run local Cargo. The new cases are expected to fail the test harness at the current source because the permit type and parameter do not exist. Exact-head remote Rust verification will execute the compiler evidence.

- [ ] **Step 3: Implement the minimal capability**

Add the opaque non-`Clone`, non-`Copy` permit and consume it as the first argument to the sole preparation entrypoint. Add no issuer, constructor, conditional compilation, runtime check, or alternate entrypoint.

- [ ] **Step 4: Update owner tests to use the same entrypoint**

Inside the existing owner-only `#[cfg(test)]` module, add one private `test_preparation_permit()` fixture that constructs the private field. Pass a fresh permit to every existing request-preparation call.

### Task 3: Evidence and Review

**Files:**
- Verify all changed files.

**Interfaces:**
- Produces: permitted local evidence, an internally reviewed diff, and explicit remote-verification status.

- [ ] **Step 1: Run focused deterministic checks**

```bash
python3 scripts/test_verify_polymarket_redemption_preparation.py
python3 scripts/verify_polymarket_redemption_preparation.py
```

- [ ] **Step 2: Run permitted repository gates**

```bash
just fmt-check
just source-fence-static
git diff --check
```

- [ ] **Step 3: Confirm the forbidden mechanisms are absent**

```bash
rg -n -F \
  -e 'import re' \
  -e 're.' \
  -e 'active production caller' \
  -e 'forbidden_observability' \
  -e 'signer_decode_markers' \
  scripts/verify_polymarket_redemption_preparation.py \
  scripts/test_verify_polymarket_redemption_preparation.py
```

Expected: no matches.

- [ ] **Step 4: Conduct internal adversarial review**

Review the complete diff for a constructible permit, alternate preparation path, runtime fallback, credential exposure, request-semantics drift, or missing deterministic evidence. Address every substantive finding before completion.

- [ ] **Step 5: Report remote evidence honestly**

Do not claim Rust compilation or behavior success until exact-head remote verification has run. Report the local checks separately from pending or completed remote evidence.
