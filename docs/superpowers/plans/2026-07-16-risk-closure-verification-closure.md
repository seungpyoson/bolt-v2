# Risk-Closure Verification Closure Implementation Plan

> **Historical reference only.** This plan is superseded by current `AGENTS.md`; its unchecked
> steps and commands are non-operational and must not be executed.

**Goal:** Replace brittle compiler-message tests and open-ended arithmetic prediction with compiler-coded evidence, recursive TOML authority enumeration, and a structural workspace-owner fence.

**Architecture:** The Rust state machine remains unchanged. The nextest-mounted compile-fail harness consumes rustc JSON diagnostics and verifies stable error codes plus a positive control. The Python verifier recursively records exact TOML key paths and limits numeric defense-in-depth to exact literals and semantic authority names.

**Tech Stack:** Rust 1.96, `serde_json`, subprocess rustc, Python 3.12 `unittest`/`tomllib`, repository source fences, governed remote nextest.

## Global Constraints

- Production activation remains disabled.
- Do not add public authority or permit constructors, storage replacement, alternate allocation, or runtime configuration paths.
- Do not change reservation, lease, permit, commit, or release state-machine behavior.
- Local agents do not run compile-heavy Rust verification; exact-head governed nextest is Rust proof.
- Do not request another external review until exact-head applicable Rust evidence is green.
- No merge, deployment, trading action, or production permit issuance.

---

### Task 1: Make compiler-negative evidence deterministic

**Files:**
- Modify: `tests/risk_closure_workspace_compile_fail.rs`

**Interfaces:**
- Consumes: the existing public opaque reservation, lease, permit, and identity types.
- Produces: `compile_snippet(case_name, body) -> Output`, `assert_compile_fails(case_name, body, expected_error_code)`, and `assert_compiles(case_name, body)`.

- [ ] **Step 1: Record RED evidence**

Use exact-head CI and the supplied Claude/Kimi compiler probes as RED evidence:

```text
reservation_private_state_cannot_be_replaced: expected privacy text, received E0509
recovery_lease_private_state_cannot_be_replaced: expected privacy text, received E0509
```

Do not locally compile Rust; the repository's Rust Probe budget is exhausted.

- [ ] **Step 2: Convert the harness to structured error codes**

Invoke rustc with `--error-format=json`. Parse each stderr line as `serde_json::Value`, collect `code.code`, and assert that the expected code exists. Preserve the complete stderr in assertion failures.

```rust
fn diagnostic_codes(stderr: &str) -> Vec<String> {
    stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|diagnostic| {
            diagnostic
                .get("code")?
                .get("code")?
                .as_str()
                .map(str::to_owned)
        })
        .collect()
}
```

Expected codes are `E0599` for missing clone, `E0382` for post-consumption use, and `E0616` for direct private-field access. Rustc currently assigns no error code to the permit struct-construction diagnostic, so that case must match the structured JSON error level, private-field span label, and private-struct-construction message shape instead of inventing an `E0451` guarantee.

- [ ] **Step 3: Probe privacy directly**

Replace functional-update syntax with direct field writes:

```rust
fn forge(mut reservation: RiskClosureWorkspaceReservation) {
    reservation.active = false;
}

fn forge(mut lease: RiskClosureWorkspaceLease) {
    lease.active = false;
}
```

Both cases expect `E0616`, proving privacy rather than the unrelated `Drop` move restriction.

- [ ] **Step 4: Add a positive control**

Add one test that must compile:

```rust
fn accepts(identity: ClosureIdentity) {
    let _ = identity.as_str();
}
```

`assert_compiles` must fail with the complete stderr if rustc, the module path, edition, or production configuration is broken.

- [ ] **Step 5: Format and commit**

Run `cargo fmt --all -- --check` and `git diff --check`. Commit:

```bash
git add tests/risk_closure_workspace_compile_fail.rs
git commit -m "test(resources): stabilize compiler authority proofs"
```

---

### Task 2: Close TOML enumeration and remove arithmetic prediction

**Files:**
- Modify: `scripts/test_verify_risk_closure_workspace_authority.py`
- Modify: `scripts/verify_risk_closure_workspace_authority.py`
- Modify: `docs/superpowers/specs/2026-07-16-risk-closure-authority-hardening-design.md`

**Interfaces:**
- Consumes: parsed TOML dictionaries/lists and repository Rust source text.
- Produces: `_toml_key_paths(value, target, prefix=()) -> list[tuple[str, ...]]`; exact `(file, key_path)` authority census; exact-literal and semantic-symbol defense-in-depth.

- [ ] **Step 1: Add recursive-census RED tests**

Add fixtures for nested dictionaries, nested occurrences in the canonical file, and arrays of tables:

```toml
[probe.risk_closure_workspaces]
capacity = 10

[[owners]]
[owners.risk_closure_workspaces]
slot_bytes = 16777216
```

Assert that each produces the `exactly one TOML authority` error. Run `python3 scripts/test_verify_risk_closure_workspace_authority.py`; expect the new nested cases to fail under the top-level-only census.

- [ ] **Step 2: Add arithmetic-boundary RED tests**

Replace expression-prediction expectations with a bounded defense-in-depth test:

```rust
const HASH_CHUNK: usize = 16 * 1024 * 1024;
```

Assert no `runtime workspace-size expression` error. Retain tests that reject exact configured integer literals and semantic names such as `RISK_CLOSURE_WORKSPACE_BYTES`, regardless of their initializer. Run the focused Python suite; expect the generic arithmetic case to fail until the evaluator is removed.

- [ ] **Step 3: Recursively enumerate exact key paths**

Walk dictionaries and lists. When a dictionary key equals `risk_closure_workspaces`, record its full key path, then continue recursively through its value. Identify authorities as `(relative_file, key_path)` and allow only `(SOURCE, ("risk_closure_workspaces",))`.

Error output must include both file and dotted/indexed key path so multiple occurrences in one file remain distinguishable.

- [ ] **Step 4: Remove the Python-AST expression evaluator**

Delete `ast`, `SIZE_EXPRESSION`, `_expression_value`, operator maps, and expression-error emission. Retain:

- exact arena/slot integer literal detection outside generated Rust;
- private owner-symbol reference detection;
- semantic `const`/`static` authority-name detection;
- generated drift checking;
- the recursive TOML census.

Update the older hardening design to describe expression checks as superseded by the structural owner boundary and link to `2026-07-16-risk-closure-verification-closure-design.md`.

- [ ] **Step 5: Verify GREEN and commit**

Run:

```bash
python3 scripts/test_verify_risk_closure_workspace_authority.py
python3 scripts/verify_risk_closure_workspace_authority.py
git diff --check
```

Expected: the focused suite and live verifier exit 0. Commit:

```bash
git add scripts/test_verify_risk_closure_workspace_authority.py scripts/verify_risk_closure_workspace_authority.py docs/superpowers/specs/2026-07-16-risk-closure-authority-hardening-design.md
git commit -m "fix(resources): enforce structural workspace authority"
```

---

### Task 3: Verify, review, publish, and stop before external review

**Files:**
- Modify only if lasting verification wording changed: PR #1430 body.

**Interfaces:**
- Consumes: Tasks 1 and 2 plus the frozen acceptance matrix.
- Produces: a clean pushed head with local non-compile evidence, internal adversarial approval, and dispatched exact-head Rust evidence.

- [ ] **Step 1: Run fresh local gates**

```bash
cargo fmt --all -- --check
git diff --check
python3 scripts/test_verify_risk_closure_workspace_authority.py
python3 scripts/verify_risk_closure_workspace_authority.py
just deny
just ci-lint-workflow
just source-fence-static
```

If a governed check fails only because the sandbox blocks its shared cache or loopback fixture, rerun that exact check with approved escalation and record both results.

- [ ] **Step 2: Conduct internal adversarial review**

Review exact commits against every row of the compiler matrix, recursive TOML key paths, structural authority boundaries, scope, and all local evidence. Resolve every substantive finding before publication.

- [ ] **Step 3: Update lasting PR wording**

Remove any claim that the Python fence evaluates common or equivalent Rust expressions. State that exact literals and semantic copied authorities are defense-in-depth while the private owner boundary is authoritative.

- [ ] **Step 4: Publish and dispatch exact-head evidence**

```bash
git push
```

Confirm remote branch HEAD exactly matches local HEAD. Advisory CI starts automatically for the pushed head. Per repository policy, detach after publication rather than waiting.

- [ ] **Step 5: Enforce the review stop**

Report pending exact-head Rust checks. Do not generate or send another external review request until a later status check confirms applicable Rust evidence, including nextest compiler-negative tests, is green.
