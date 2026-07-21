# Risk-Closure Authority Hardening Implementation Plan

> **Historical reference only.** This plan is superseded by current `AGENTS.md`; its unchecked
> steps and commands are non-operational and must not be executed.

**Goal:** Close the confirmed PR #1430 authority, permit-generation, memory-bound, fence, and verification findings without activating production behavior.

**Architecture:** Keep one preallocated per-slot workspace owner, remove production construction and replacement entrypoints, and bind every release permit to an opaque authority instance plus closure generation. Expand the existing Python authority fence to govern both configured byte values and repository TOML, while keeping generator tests on the existing discovered verifier path.

**Tech Stack:** Rust 1.96, `Arc`/`Mutex`, Python 3 `unittest`/`tomllib`/`ast`, Cargo nextest remote CI, repository source fences.

## Global Constraints

- Production activation remains disabled.
- No public authority constructor, storage replacement path, terminal-permit constructor, boolean shortcut, or alternate allocation route.
- `release_terminal` consumes both authorities on success and returns both intact on every failure.
- Runtime Rust compilation and tests run only through exact-head remote CI; local checks are formatting, Python tests, dependency policy, CI lint, and `source-fence-static`.
- Rust Probe is not available for this slice because the branch has already consumed its two-run limit.
- No merge, deployment, trading action, or production permit issuance.

---

### Task 1: Bind authority and closure generation

**Files:**
- Modify: `src/bolt_v3_risk_closure_workspace.rs`

**Interfaces:**
- Consumes: private `WorkspaceState`, `SlotState`, `RiskClosureWorkspaceLease`, `TerminalReleasePermit`.
- Produces: private `AuthorityIdentity(Arc<()>)`; retained `closure_generation: u64`; permits bound to authority identity, closure identity, and generation.

- [ ] **Step 1: Add failing behavioral tests before production changes**

Add unit tests that construct two private test authorities with the same closure identity and prove a permit from the first cannot release the second. Add a same-authority identity-reuse test that creates two old-generation test permits, releases the old closure with one, reuses the identity, and proves the remaining old permit is rejected while both returned authorities remain recoverable.

```rust
#[test]
fn terminal_permit_cannot_cross_authority_instances() {
    let first = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
    let second = RiskClosureWorkspaceAuthority::with_config(test_config()).unwrap();
    let closure_identity = identity(usize::default());
    first.checkout_new_risk().unwrap().commit(closure_identity.clone()).unwrap();
    second.checkout_new_risk().unwrap().commit(closure_identity.clone()).unwrap();
    let first_lease = first.checkout_recovery(&closure_identity).unwrap();
    let second_lease = second.checkout_recovery(&closure_identity).unwrap();
    let first_permit = terminal_permit(&first_lease);
    let failure = second_lease.release_terminal(first_permit).unwrap_err();
    assert_eq!(failure.error(), RiskClosureWorkspaceError::LeaseIdentityMismatch);
    let (second_lease, first_permit) = failure.into_parts();
    drop(second_lease);
    first_lease.release_terminal(first_permit).unwrap();
}
```

Rust RED evidence is the reviewer-proven cross-authority/stale-generation scenario. Do not locally compile Rust under the remote-first policy.

- [ ] **Step 2: Implement opaque authority and generation binding**

Add a private pointer-identity token and carry it through state, leases, test durable transitions, and permits. Retain the committing reservation's unique lease ID as the closure generation.

```rust
#[derive(Debug, Clone)]
struct AuthorityIdentity(Arc<()>);

impl AuthorityIdentity {
    fn new() -> Self { Self(Arc::new(())) }
    fn matches(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
}

struct TerminalReleasePermit {
    authority_identity: AuthorityIdentity,
    closure_identity: ClosureIdentity,
    closure_generation: u64,
}
```

Validate all three bindings before mutating `logical_slots` or `slots`. Return `TerminalReleaseFailure::new(RiskClosureWorkspaceError::LeaseIdentityMismatch, self, permit)` on mismatch.

- [ ] **Step 3: Remove multiplicity and peak-memory routes**

Delete `RiskClosureWorkspaceAuthority::new`, `replace_storage_from_generated_config`, `replace_storage`, and the four unused configuration accessors. Keep `with_config` private and gated with `#[cfg(test)]`; production code in this slice cannot construct an authority.

Delete storage-replacement tests and rewrite compile-fail/doc snippets to accept reservation or lease values as function parameters instead of constructing an authority.

- [ ] **Step 4: Add actual generated-allocation evidence**

Add one unit test using `RISK_CLOSURE_WORKSPACE_CONFIG` that asserts `reserved_bytes == arena_bytes`, checks out exactly `capacity` reservations, and observes `CapacityExhausted` on the next checkout. This allocates and touches the configured ten independent buffers in governed remote CI.

- [ ] **Step 5: Format and commit Task 1**

Run:

```bash
cargo fmt --all
cargo fmt --all -- --check
git diff --check
```

Expected: all commands exit 0. Then commit:

```bash
git add src/bolt_v3_risk_closure_workspace.rs tests/risk_closure_workspace_compile_fail.rs
git commit -m "fix(resources): bind terminal permits to closure generations"
```

### Task 2: Govern complete configured geometry and TOML census

**Files:**
- Modify: `scripts/verify_risk_closure_workspace_authority.py`
- Modify: `scripts/test_verify_risk_closure_workspace_authority.py`
- Delete: `scripts/test_generate_risk_closure_workspace_config.py`

**Interfaces:**
- Consumes: sole TOML fields `arena_bytes` and `slot_bytes`; existing `authority_errors(root)` entrypoint.
- Produces: repository-wide TOML census and value checks for both configured byte values through the already discovered verifier suite.

- [ ] **Step 1: Add failing fence tests**

Add fixtures for a duplicate authority at `crates/consumer/runtime.toml`, a direct `arena_bytes` literal, and an evaluated arena expression. Move the four generator tests into a second `unittest.TestCase` in the governed verifier test file.

```python
def test_rejects_arena_size_literal(self) -> None:
    (self.root / "src" / "consumer.rs").write_text(
        "const PLANTED_ARENA_TOTAL: usize = 167_772_160;\n",
        encoding="utf-8",
    )
    errors = verifier.authority_errors(self.root)
    self.assertTrue(any("runtime workspace-size literal" in error for error in errors))
```

Run `python3 scripts/test_verify_risk_closure_workspace_authority.py` and confirm the new census and arena tests fail for the missing protections.

- [ ] **Step 2: Implement repository TOML census and both-value checks**

Read every `*.toml` below the supplied root while excluding `.git`, `.worktrees`, and `target` path components. Capture both positive integer values from `config/risk-closure-workspaces.toml` and compare literals/expressions against a set:

```python
authoritative_sizes = {authoritative_arena_bytes, authoritative_slot_bytes}
if any(_integer_value(match.group()) in authoritative_sizes for match in INTEGER_LITERAL.finditer(text)):
    errors.append(f"runtime workspace-size literal found outside generated Rust: {relative}")
```

Keep capacity derived and private rather than value-fencing the common integer `10`.

- [ ] **Step 3: Consolidate generator tests**

Import `generate_risk_closure_workspace_config as generator` from the governed test file, move its geometry/schema/activation cases there unchanged, and delete `scripts/test_generate_risk_closure_workspace_config.py`.

- [ ] **Step 4: Verify and commit Task 2**

Run:

```bash
python3 scripts/test_verify_risk_closure_workspace_authority.py
python3 scripts/verify_risk_closure_workspace_authority.py
just source-fence-static
```

Expected: all verifier tests and fences exit 0. Then commit:

```bash
git add scripts/verify_risk_closure_workspace_authority.py scripts/test_verify_risk_closure_workspace_authority.py
git add -u scripts/test_generate_risk_closure_workspace_config.py
git commit -m "fix(resources): govern complete workspace geometry"
```

### Task 3: Complete compiler-negative API coverage

**Files:**
- Modify: `tests/risk_closure_workspace_compile_fail.rs`

**Interfaces:**
- Consumes: public opaque reservation, lease, and permit types.
- Produces: compiler failures for permit post-consumption reuse and private-field construction without adding a Cargo test target.

- [ ] **Step 1: Add compiler-negative cases**

Add a function-parameter snippet that calls `release_terminal(permit)` and then attempts `permit.clone()` or reuse, expecting `use of moved value: permit`. Add reservation and lease struct-literal construction snippets and assert stable privacy diagnostics.

```rust
fn release(lease: RiskClosureWorkspaceLease, permit: TerminalReleasePermit) {
    lease.release_terminal(permit).unwrap();
    let _reuse = permit;
}
```

Do not replace `Command::new("rustc")` with `env!("RUSTC")`; Cargo does not guarantee that compile-time variable. The exact-head governed nextest run is the compiler/toolchain evidence.

- [ ] **Step 2: Format and commit Task 3**

Run `cargo fmt --all -- --check` and `git diff --check`; expect exit 0. Commit:

```bash
git add tests/risk_closure_workspace_compile_fail.rs
git commit -m "test(resources): close terminal authority compile gaps"
```

### Task 4: Verify, review, and publish exact head

**Files:**
- Modify only if lasting behavior changed: PR #1430 body.

**Interfaces:**
- Consumes: completed Tasks 1-3.
- Produces: clean pushed exact head with local non-compile evidence, internal adversarial review, and dispatched remote proof.

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

Expected: every applicable command exits 0. If a sandbox-only loopback/cache permission fails, rerun that exact governed command with approved escalation and record both results.

- [ ] **Step 2: Conduct internal adversarial review**

Review the exact commit range for authority multiplicity, temporary double allocation, authority/generation permit binding, duplicate commit atomicity, test discovery, TOML census, both configured byte values, and compiler-test validity. Resolve every substantive finding before publication.

- [ ] **Step 3: Publish and dispatch remote evidence**

```bash
git push
```

Confirm the pushed SHA equals remote branch HEAD. Advisory CI starts automatically for the pushed head. Update the stable PR body to remove storage-replacement claims and describe generation-bound permits. Per repository policy, detach after publication rather than waiting on CI; report exact-head pending checks without claiming Rust success.
