# Redemption Review Findings Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the three internal adversarial-review findings without activating or otherwise expanding the disabled Polymarket redemption-preparation slice.

**Architecture:** Strengthen the existing Python source fence so it conservatively scans code in every repository Rust source, validates the owner's final cfg(test) boundary, and rejects unsafe signer parsing and direct output macros. Replace the signer parser with an optional-prefix-compatible decode into a zeroizing fixed-size buffer, and positively require that decode structure in the preparation function.

**Tech Stack:** Python 3.11 `unittest`/`tomllib`, Rust, `alloy_primitives::hex`, `alloy_signer_local::PrivateKeySigner`, `zeroize::Zeroizing`, repository `just` verification recipes.

## Global Constraints

- No redemption activation, network submission, durable authority, or production caller.
- No new configuration fields, secret sources, dependencies, or compatibility paths.
- Preserve calldata, signing-domain, signature-packing, retry, lease, callback, and identity semantics.
- Do not run local compile-heavy Rust verification; use exact-head remote Rust verification.
- Every fence change receives a mutation test that is observed failing before its implementation.

---

### Task 1: Complete Production-Caller Discovery

**Files:**
- Modify: `scripts/test_verify_polymarket_redemption_preparation.py:127-342`
- Modify: `scripts/verify_polymarket_redemption_preparation.py:55-88,376-386`

**Interfaces:**
- Consumes: repository `Cargo.toml` files and the existing `_production_owner(text: str) -> str` helper.
- Produces: `_production_rust_sources(root: pathlib.Path) -> tuple[list[pathlib.Path], list[str]]`, returning sorted package production sources and fail-closed inspection errors.

- [ ] **Step 1: Make the verifier fixture a package and add failing owner/nested-package mutations**

Add a `[package]` table to the fixture manifest. Add one test that inserts a second production reference before `#[cfg(test)]` in the owner, and one that creates `crates/consumer/Cargo.toml` plus `crates/consumer/src/lib.rs` containing a qualified `bolt_v2::bolt_v3_polymarket_redemption::prepare_redemption_request` reference. Both tests assert an `active production caller` error.

- [ ] **Step 2: Run the focused tests and verify RED**

Run from `scripts/`:

```bash
python3 -m unittest \
  test_verify_polymarket_redemption_preparation.PolymarketRedemptionPreparationVerifierTests.test_active_caller_in_owner_is_rejected \
  test_verify_polymarket_redemption_preparation.PolymarketRedemptionPreparationVerifierTests.test_active_caller_in_nested_package_is_rejected
```

Expected: two assertion failures because the owner is excluded and only root `src` is scanned.

- [ ] **Step 3: Implement manifest-derived production source discovery**

Add a helper that:

```python
def _production_rust_sources(
    root: pathlib.Path,
) -> tuple[list[pathlib.Path], list[str]]:
    sources: set[pathlib.Path] = set()
    errors: list[str] = []
    manifests = (path for path in _repository_toml(root) if path.name == "Cargo.toml")
    for manifest in manifests:
        try:
            cargo = _toml(manifest)
        except (OSError, UnicodeDecodeError, tomllib.TOMLDecodeError) as error:
            errors.append(f"cannot inspect Cargo package {manifest.relative_to(root)}: {error}")
            continue
        package = cargo.get("package")
        if package is None:
            continue
        if not isinstance(package, dict):
            errors.append(f"Cargo package table must be a table: {manifest.relative_to(root)}")
            continue
        package_root = manifest.parent
        source_root = package_root / "src"
        if source_root.is_dir():
            sources.update(
                path
                for path in source_root.rglob("*.rs")
                if "tests" not in path.relative_to(package_root).parts
            )
        explicit_targets: list[tuple[str, object]] = []
        if "lib" in cargo:
            explicit_targets.append(("lib", cargo["lib"]))
        bins = cargo.get("bin", [])
        if not isinstance(bins, list):
            errors.append(f"Cargo bin targets must be an array: {manifest.relative_to(root)}")
            continue
        explicit_targets.extend((f"bin[{index}]", target) for index, target in enumerate(bins))
        for label, target in explicit_targets:
            if not isinstance(target, dict):
                errors.append(
                    f"Cargo {label} target must be a table: {manifest.relative_to(root)}"
                )
                continue
            target_path = target.get("path")
            if target_path is None:
                continue
            if not isinstance(target_path, str):
                errors.append(
                    f"Cargo {label} target path must be a string: {manifest.relative_to(root)}"
                )
                continue
            candidate = package_root / target_path
            if not candidate.is_file():
                errors.append(
                    f"cannot inspect Cargo {label} target {candidate.relative_to(root)}"
                )
                continue
            sources.add(candidate)
    return sorted(sources), errors
```

Use it in `boundary_errors`. Inspect the production portion of every discovered source except generated output and the owner. For the owner, require exactly one production occurrence of `prepare_redemption_request`, its declaration.

- [ ] **Step 4: Run focused and full verifier tests and verify GREEN**

Run from `scripts/`:

```bash
python3 -m unittest test_verify_polymarket_redemption_preparation
```

Expected: all tests pass.

- [ ] **Step 5: Commit the caller-fence change**

```bash
git add scripts/verify_polymarket_redemption_preparation.py scripts/test_verify_polymarket_redemption_preparation.py
git commit -m "fix: close redemption caller fence"
```

- [ ] **Step 6: Adversarially mutate path, cfg, target, and generated boundaries**

Add mutations for a caller in `src/tests/active.rs`, after the owner's test module, after a non-owner cfg(test) item, in a custom `[lib] path` module descendant, and in generated Rust. Run the five focused tests and observe five failures from the manifest/path implementation.

- [ ] **Step 7: Replace path inference with conservative Rust-code inspection**

Enumerate every repository Rust source without a `tests` or generated-source exemption:

```python
def _repository_rust_sources(root: pathlib.Path) -> list[pathlib.Path]:
    ignored = {".git", ".worktrees", "target"}
    return sorted(
        path
        for path in root.rglob("*.rs")
        if not ignored.intersection(path.relative_to(root).parts)
    )
```

Use lexical scanning that skips nested comments, raw/quoted strings, and character literals before matching `prepare_redemption_request`. Require the owner's one exact `#[cfg(test)] mod tests` item to have a balanced lexical brace span and only whitespace after its closing brace; inspect the prefix for the one declaration.

- [ ] **Step 8: Verify the adversarial caller mutations and repository are GREEN**

Run the five focused tests, the full verifier test module, and `python3 verify_polymarket_redemption_preparation.py ..`. Expected: all pass.

---

### Task 2: Reject Direct Secret-Output Macros

**Files:**
- Modify: `scripts/test_verify_polymarket_redemption_preparation.py:343-385`
- Modify: `scripts/verify_polymarket_redemption_preparation.py:336-347`

**Interfaces:**
- Consumes: the production owner text already isolated by `_production_owner`.
- Produces: a forbidden-observability check covering `dbg!`, `print!`, and `eprint!` in addition to existing sinks.

- [ ] **Step 1: Add a failing mutation test for each output macro**

For each of these compile-plausible production snippets, insert it before `#[cfg(test)]`, assert a `forbidden logging or observability sink` error, then restore the owner fixture:

```rust
fn leak(credentials: &ResolvedRedemptionCredentials) { dbg!(&credentials.signer_private_key); }
fn leak(credentials: &ResolvedRedemptionCredentials) { print!("{}", credentials.signer_private_key.as_str()); }
fn leak(credentials: &ResolvedRedemptionCredentials) { eprint!("{}", credentials.signer_private_key.as_str()); }
```

- [ ] **Step 2: Run the focused test and verify RED**

Run from `scripts/`:

```bash
python3 -m unittest test_verify_polymarket_redemption_preparation.PolymarketRedemptionPreparationVerifierTests.test_direct_secret_output_macros_are_rejected
```

Expected: assertion failure for the first unrecognized macro.

- [ ] **Step 3: Extend the forbidden-observability tuple**

Add the exact tokens `dbg!`, `print!`, and `eprint!` to `forbidden_observability`.

- [ ] **Step 4: Run the focused test and verify GREEN**

Run the Step 2 command again. Expected: pass.

- [ ] **Step 5: Commit the output-fence change**

```bash
git add scripts/verify_polymarket_redemption_preparation.py scripts/test_verify_polymarket_redemption_preparation.py
git commit -m "fix: fence redemption secret output macros"
```

---

### Task 3: Keep Decoded Private-Key Bytes Zeroizing

**Files:**
- Modify: `scripts/test_verify_polymarket_redemption_preparation.py:343-400`
- Modify: `scripts/verify_polymarket_redemption_preparation.py:320-350`
- Modify: `src/bolt_v3_polymarket_redemption.rs:1-6,271-276`

**Interfaces:**
- Consumes: `credentials.signer_private_key: Zeroizing<String>` and the existing `InvalidSigningKey` error.
- Produces: a `PrivateKeySigner` constructed from a `Zeroizing<[u8; 32]>` decode buffer; accepted `0x`-prefixed input remains supported.

- [ ] **Step 1: Add a failing unsafe-parser mutation test**

Insert `PrivateKeySigner::from_str(credentials.signer_private_key.as_str())` into the production owner fixture and assert an error containing `zeroizing signer-key decode`.

- [ ] **Step 2: Run the focused test and verify RED**

Run from `scripts/`:

```bash
python3 -m unittest test_verify_polymarket_redemption_preparation.PolymarketRedemptionPreparationVerifierTests.test_nonzeroizing_signer_parser_is_rejected
```

Expected: assertion failure because the verifier does not yet reject the parser.

- [ ] **Step 3: Add the unsafe-parser source fence**

Reject `PrivateKeySigner::from_str` in the production owner with the error `production owner must use a zeroizing signer-key decode buffer`.

- [ ] **Step 4: Verify the unit test is GREEN and the real repository is RED**

Run from `scripts/`:

```bash
python3 -m unittest test_verify_polymarket_redemption_preparation.PolymarketRedemptionPreparationVerifierTests.test_nonzeroizing_signer_parser_is_rejected
python3 verify_polymarket_redemption_preparation.py ..
```

Expected: the unit test passes; repository verification fails on the current `PrivateKeySigner::from_str` call.

- [ ] **Step 5: Replace the signer parser with a zeroizing decode buffer**

Remove `str::FromStr`, import `alloy_primitives::hex`, and replace the parser with:

```rust
let mut signer_private_key = Zeroizing::new(B256::ZERO.into_array());
hex::decode_to_slice(
    credentials.signer_private_key.as_bytes(),
    signer_private_key.as_mut(),
)
.map_err(|_| RedemptionPreparationError::InvalidSigningKey)?;
let signer = PrivateKeySigner::from_slice(signer_private_key.as_ref())
    .map_err(|_| RedemptionPreparationError::InvalidSigningKey)?;
```

- [ ] **Step 6: Run focused repository verification and verify GREEN**

Run from `scripts/`:

```bash
python3 -m unittest test_verify_polymarket_redemption_preparation
python3 verify_polymarket_redemption_preparation.py ..
```

Expected: both commands pass.

- [ ] **Step 7: Commit the zeroizing decode change**

```bash
git add src/bolt_v3_polymarket_redemption.rs scripts/verify_polymarket_redemption_preparation.py scripts/test_verify_polymarket_redemption_preparation.py
git commit -m "fix: zeroize redemption signer decode"
```

- [ ] **Step 8: Add positive signer-decode mutations after adversarial review**

Replace `from_slice` with `.parse::<PrivateKeySigner>()`, replace it with an aliased `FromStr::from_str`, and remove `Zeroizing::new` from the decode buffer. Observe all three focused subtests fail before strengthening the verifier.

- [ ] **Step 9: Require the secure decode shape in the preparation function**

Extract the full `prepare_redemption_request` function with the lexical brace scanner. Require exactly one occurrence of each marker, in this order: the `Zeroizing::new(B256::ZERO.into_array())` buffer, `hex::decode_to_slice`, the credential bytes input, the mutable zeroizing buffer, and `PrivateKeySigner::from_slice` over that buffer. Reject direct `from_str`, `.parse::<PrivateKeySigner>`, and aliased `FromStr` spellings.

- [ ] **Step 10: Verify the signer mutations and repository are GREEN**

Run the focused signer tests, the full verifier test module, and `python3 verify_polymarket_redemption_preparation.py ..`. Expected: all pass.

---

### Task 4: Verification and Exact-Head Evidence

**Files:**
- Verify only; no planned source changes.

**Interfaces:**
- Consumes: all three fix commits.
- Produces: local non-compile evidence and exact-head remote Rust evidence.

- [ ] **Step 1: Run formatting and targeted Python verification**

```bash
just fmt-check
python3 scripts/test_verify_polymarket_redemption_preparation.py
python3 scripts/verify_polymarket_redemption_preparation.py
```

Expected: every command exits zero.

- [ ] **Step 2: Run governed static/source fences**

```bash
just source-fence-static
```

Expected: exit zero with the redemption verifier and its mutation tests passing.

- [ ] **Step 3: Inspect the final diff and worktree**

```bash
git diff origin/main...HEAD --check
git status --short --branch
```

Expected: no whitespace errors and no uncommitted files.

- [ ] **Step 4: Publish through the sandbox-safe path and request exact-head remote verification**

```bash
just sandbox-safe-push
just verify-remote
```

Expected: the remote branch resolves to the exact local `HEAD`; remote verification is dispatched for that SHA. Do not wait on CI. Report the head SHA and detach per repository policy.
