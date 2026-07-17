# Redemption Review Findings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace predictive Rust-source fencing with one compiler-enforced, one-use capability while retaining the zeroizing signer fix and deterministic configuration evidence.

**Architecture:** `prepare_redemption_request` consumes an opaque permit that safe production code cannot construct and borrows the retained-recovery lease re-exported by the existing `ApplicationResourceLedger`. The Python verifier is reduced to parsed TOML, exact pins, hashes, manifest wiring, and generator drift; Rust compile-fail and behavior tests own Rust-language evidence.

**Tech Stack:** Rust, Python 3.11 `unittest`/`tomllib`, repository `just` verification recipes.

## Global Constraints

- No regex or Rust-source prediction.
- No redemption activation, submission, durable authority, or production caller.
- No fallback entrypoint, compatibility path, caller allowlist, or runtime branch for permit issuance.
- Preserve request bytes, signing, retry, lease, callback, and identity behavior.
- Do not run local compile-heavy Rust verification; use exact-head remote Rust evidence.
- Do not restore the retired top-level risk-workspace module or create a second authority.

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

### Task 4: Close accepted PR review findings

**Files:**
- Modify: `src/bolt_v3_providers/polymarket.rs`
- Modify: `src/bolt_v3_polymarket_redemption.rs`
- Modify: `tests/polymarket_redemption_preparation_compile_fail.rs`
- Modify: `config/polymarket-redemption.toml`
- Modify: `config/polymarket-redemption-source-evidence.toml`
- Modify: generator, verifier, generated projection, and focused tests.

**Interfaces:**
- Consumes: `ResolvedEvmSigningKey`, an opaque checked signing value supplied directly to the disabled primitive.
- Produces: disabled request preparation with no independent credential resolver.

- [ ] **Step 1: Update regression evidence**

Change owner and compile-fail tests to pass `ResolvedEvmSigningKey`, add an oversized-nonce classification assertion, and make generator/verifier fixtures reject `output_asset` as an unknown field.

- [ ] **Step 2: Remove the duplicate credential path**

Delete `ResolvedRedemptionCredentials`, its SSM resolver, and its duplicate secret validation. Keep provider resolution as the single SSM path and validate the resolved key there without retaining a second representation. Make `prepare_redemption_request` borrow a checked `ResolvedEvmSigningKey` and copy its key bytes into `Zeroizing<[u8; 32]>` before signer construction.

- [ ] **Step 3: Close the mechanical findings**

Return `InvalidRequestInput { field: "safe_nonce" }` for an over-bound nonce, resolve compile-test Cargo through `PATH`, remove `output_asset` end-to-end, bump the deployment-fact payload format, regenerate Rust, and remove the accidental issue-closing phrase from the PR body.

- [ ] **Step 4: Verify and publish**

Run the focused Python suites, generator/verifier checks, formatting, source-fence static checks, and `git diff --check`. Conduct an internal adversarial diff review, commit, publish with `just sandbox-safe-push`, and report the new exact head without waiting on CI.

Do not claim Rust compilation or behavior success until exact-head remote verification has run. Report the local checks separately from pending or completed remote evidence.

### Task 5: Port onto the authoritative application resource ledger

**Files:**
- Modify: `src/bolt_v3_application_resource_ledger.rs`
- Modify: `src/bolt_v3_polymarket_redemption.rs`
- Modify: `tests/polymarket_redemption_preparation_compile_fail.rs`
- Modify: `src/lib.rs`
- Modify: PR #1439 body

**Interfaces:**
- Consumes: `bolt_v3_application_resource_ledger::RiskClosureWorkspaceLease`.
- Preserves: `ApplicationResourceLedger` as the only production owner of workspace authority.

- [ ] **Step 1: Port the accepted slice from current `main`**

Apply the reviewed redemption files to a fresh branch rooted at current `main`. Do not port the retired `src/bolt_v3_risk_closure_workspace.rs` modification.

- [ ] **Step 2: Bind tests and production code to the ledger export**

Change imports to `bolt_v3_application_resource_ledger`. Add only a `#[cfg(test)] pub(crate)` ledger helper that obtains a retained-recovery lease through the ledger's new-risk and recovery handles; add no production ledger constructor or alternate lease path.

- [ ] **Step 3: Correct review documentation and PR scope**

Describe the provider's single zeroizing string representation and checked signing-key value accurately. State that #1384 still owns their production binding, output-asset/post-state binding, and live redemption work, while #1382/#1441 already supplied application-resource-ledger ownership.

- [ ] **Step 4: Verify and publish without rewriting the PR branch**

Run focused Python tests, generator/verifier checks, formatting, source-fence static checks, and `git diff --check`. Commit the fresh current-main tree, merge the old PR head only as a history parent while retaining the fresh tree, then publish to PR #1439's branch with `just sandbox-safe-push --branch codex/1384-disabled-redemption-preparation`. Report exact-head Rust CI as pending until remote evidence runs.

### Task 6: Seal the fixed-width signing-key value

**Files:**
- Modify: `src/bolt_v3_secrets.rs`
- Modify: `src/bolt_v3_providers/polymarket.rs`
- Modify: `src/bolt_v3_providers/mod.rs`
- Modify: `src/bolt_v3_polymarket_redemption.rs`

**Interfaces:**
- Produces: `ResolvedEvmSigningKey::new(Zeroizing<[u8; 32]>) -> Result<Self, String>`.
- Removes: `ResolvedEvmSigningKey::from_bytes([u8; 32])`.

- [ ] Add a provider unit test proving an invalid scalar cannot construct `ResolvedEvmSigningKey`; record remote Rust RED because the current head does not compile and local compile-heavy Rust is forbidden.
- [ ] Allocate `Zeroizing<[u8; 32]>` before copying NT-validated bytes, validate inside the signing-key constructor, and return `&self.bytes` from the fixed-width accessor.
- [ ] Route fixtures through the checked constructor or provider decode, copy request bytes into an already-zeroizing buffer, and remove the unused production `ZeroizeOnDrop` import.
- [ ] Use exact-head remote build, clippy, Backtester, behavior, and compile-fail execution as GREEN evidence.

### Task 6A: Remove the premature provider signer field

**Files:**
- Modify: `src/bolt_v3_providers/polymarket.rs`
- Modify: provider fixtures and redemption owner tests.

**Interfaces:**
- Preserves: `ResolvedBoltV3PolymarketSecrets` as one zeroizing string representation per resolved secret.
- Preserves: `ResolvedEvmSigningKey` as the checked direct input to request preparation.

- [ ] Delete the stored `redemption_signing_key` field and dead accessor so external fixtures remain constructible and one logical secret cannot diverge across two fields.
- [ ] Keep provider resolution validation through `decode_private_key`, then discard the temporary checked value instead of retaining a parallel representation.
- [ ] Pass a separately checked owner-test key directly to the disabled preparation primitive; leave production provider binding to the remaining #1384 scope.

### Task 7: Remove unused credential authorities

**Files:**
- Modify: `config/polymarket-redemption.toml`
- Modify: `scripts/generate_polymarket_redemption_config.py`
- Modify: `scripts/verify_polymarket_redemption_preparation.py`
- Modify: both focused Python test suites.

**Interfaces:**
- Retains: `wallet_authority.root_client` solely to derive the SAFE wallet identity.
- Removes: redemption-owned builder credential paths, AWS region, and signer SSM path.

- [ ] Add failing tests that reject `[credential_set]` as an unknown runtime table and prove only `safe_address` is derived from the selected root client.
- [ ] Delete the dead runtime fields, parser branches, authority-census entries, fixtures, and comments.
- [ ] Run both focused Python suites and the deterministic verifier to GREEN.

### Task 8: Derive the selector and own external evidence bytes

**Files:**
- Create: `scripts/ethereum_keccak.py`
- Create: `config/polymarket-redemption-sources/**`
- Modify: `config/polymarket-redemption-source-evidence.toml`
- Modify: generator, verifier, generated projection, runtime-literal audit, and focused tests.

**Interfaces:**
- Produces: `ethereum_keccak.keccak_256(data: bytes) -> bytes`.
- Consumes: source metadata whose immutable capture paths are derived from URL/repository, revision, source path, and observation date, plus declared SHA-256 values.

- [ ] Add known-answer RED tests for Keccak-256 of empty bytes and the canonical redemption signature; add mutation tests showing a handwritten selector is rejected and a changed signature changes the projection.
- [ ] Add RED tests for missing or mutated derived captures, invalid or parent-traversing source metadata, and deployment facts that disagree with the captured official Markdown.
- [ ] Implement the dependency-free Keccak permutation and derive the four-byte selector from `function_signature`.
- [ ] Vendor exact-byte hexadecimal captures of the five pinned source files and official contracts Markdown, derive every capture path from provenance metadata, bind each decoded capture to its SHA-256, and structurally compare chain ID plus three deployment addresses to runtime TOML.
- [ ] Remove the self-referential deployment-fact hash and independently editable selector authority; regenerate Rust and run focused suites to GREEN.

### Task 9: Publish honest exact-head evidence

**Files:**
- Modify: PR #1439 body.
- Verify: complete changed-file set.

- [ ] Rename the PR body's validation section to merge requirements and avoid claiming transient exact-head results.
- [ ] Run `just fmt-check`, `just source-fence-static`, focused Python suites, generator `--check`, deterministic verification, and `git diff --check`.
- [ ] Conduct an internal adversarial review for another signer constructor, dead authority, selector copy, unverified snapshot, network fallback, or stale PR scope.
- [ ] Commit and publish with `just sandbox-safe-push --branch codex/1384-disabled-redemption-preparation`; report the exact head and leave Rust verification to the new remote run.
