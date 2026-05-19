# Research: Disk Pressure Governance

## Decision: Treat #123 as an epic governance slice, not one implementation PR

**Rationale**: The live issue body maps nine child tracks with different owners and states. #70 is closed by investigation. #125 is a bolt-v2 incident anchor whose implementation continues in `claude-config`. #286 is complete via PR #404, and #48/#124 residual routing now lives in #374 or existing evidence. A single code PR claiming to close #123 would violate one-issue-one-PR scope discipline.

**Alternatives considered**:
- One large "fix disk pressure" PR: rejected because it would collapse distinct symptoms and hide residual scope.
- Close #123 after comments only: rejected because implementation owners still remain open.

## Decision: #374, #375, and #377 must start with Phase 1 MECE enumeration

**Rationale**: Their May 17 comments explicitly block implementation until enumeration is present, reviewed for gaps/overlaps, and pinned. For #374 this covers cargo invocation paths. For #375 this covers developer-tool disk writers. For #377 this covers detection surface and failure modes.

**Alternatives considered**:
- Implement shim/log rotation/detection immediately: rejected because current issue comments say those proposals are Phase 3 draft until Phase 1 is complete.

## Decision: Keep #286 complete and route wrapper residuals to #374

**Rationale**: PR #404 merged the managed Rust cache status/prune policy into main at `400dac8acc8ec04fc7b4aefc41bab10390d6404f` and closed #286. External review accepted narrow active-process wrapper limitations as follow-up inventory, not #286 blockers. Keeping those edges in #374 avoids dual ownership for the same cargo-routing surface.

**Explicit #374 residuals from #404 review**:
- `env -iuLD_PRELOAD cargo build`
- `rustup run stable -- -- cargo build`
- depth-cap observability for deeply wrapped process detection
- wrapper inventory for `timeout`, `xargs`, `setsid`, `taskset`, `ionice`, `chrt`, `make`, `python -c` / `os.system(...)`, and symlink-renamed `cargo` or `rustc`

**Alternatives considered**:
- Re-open #286 for wrapper inventory: rejected because #286 owns managed-cache retention, while #374 owns cargo invocation and wrapper hardening.
- Treat accepted wrapper edges as untracked: rejected because they are concrete review findings and must remain visible.

## Decision: Use CI as the broad verifier and local Cargo only by exception

**Rationale**: `just test` delegates through `rust_verification.py run --repo ... test`, and CI already has a nextest archive/cache shape for full-suite lanes. Broad local Cargo duplicates mandatory CI proof and consumes laptop disk. Local Cargo remains valid for narrow TDD, CI-failure reproduction, and testing the local routing/cache behavior itself.

**Alternatives considered**:
- Always run full local `cargo test`: rejected because broad local execution duplicates CI, can recreate the symptom, and has high disk cost.
- Never run local Cargo: rejected because narrow TDD and local-routing work need fast local feedback.
- Open draft PR early and let CI run broad validation: accepted as the default broad-verification path.

## Decision: Treat no-mistakes as a verified Cargo-routing gap

**Rationale**: For operator `spson` on 2026-05-18, no-mistakes v1.18.3 was running without `CARGO_TARGET_DIR` in the daemon environment. The repo `.no-mistakes.yaml` configures raw `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`. A recorded bolt-v2 no-mistakes run wrote Cargo output under `USER_HOME_DIR/.no-mistakes/worktrees/WORKTREE_HASH/WORKTREE_ID/target/...` and failed with `No space left on device`. This proves no-mistakes is not only theoretical drift; it is a real unmanaged target producer.

**Alternatives considered**:
- Assume no-mistakes inherits managed routing: rejected by daemon env check and historical failure path.
- Delete no-mistakes worktrees after failure: rejected as a one-off cleanup that does not prevent recurrence.
- Route no-mistakes to managed commands or exact-head CI evidence: accepted as the implementation direction for #374.

## Decision: Do not use S3 as active Cargo target cache

**Rationale**: Cargo target directories are mutable, high-churn, and sensitive to compiler/profile/target fingerprints. S3 is object storage, not a local filesystem with safe concurrent mutation semantics. The repo already uses S3 for deploy artifacts, which is appropriate for immutable reviewed outputs, not active build products.

**Alternatives considered**:
- Mount S3 or sync target dirs to S3: rejected because latency, consistency, and concurrent write hazards would add a second build path.
- Use S3 only for immutable artifacts/evidence: accepted.

## Decision: Dry-run-first cleanup is mandatory

**Rationale**: The epic includes live process and file-holder hazards: unlinking active Claude `.output` files does not reclaim space, managed cache is useful hot cache, and toolchain removal can break active builds. Dry-run output plus active-process refusal is the safe baseline.

**Alternatives considered**:
- Periodic blind `rm -rf`: rejected because it is destructive, can fail to reclaim live files, and can break current work.

## Decision: External review gates come after local proof and exact-head CI

**Rationale**: Repo review rules prohibit external review while branch state, local findings, or checks are unresolved. no-mistakes and external models are useful only after the PR head is clean and verified.

**Alternatives considered**:
- Ask all models to review draft code before tests/CI: rejected because it produces noisy findings against unstable state.
