# Log Ignore Policy Audit Design

## Status

Draft design for review.

## Scope

This spec defines the follow-up work for issue `#72`: deciding whether `bolt-v2` should keep a broad `*.log` ignore rule or replace it with explicit log artifact patterns.

It is intentionally narrow:

- inventory real `.log` producers relevant to the main checkout and official `git worktree`s for this repository
- preserve current runtime behavior
- change `.gitignore` only if explicit patterns can be proven safe

This spec does **not** cover:

- changing where runtime logs are written
- introducing new CLI or TOML settings for logging directories
- broader source-tree cleanup or module reorganization

## Evidence Baseline

### Current Ignore Policy

The repository currently ignores:

- `/target`
- `config/live.toml`
- `config/live.local.toml`
- `*.log`
- `.omx/`
- `.claude/findings/`
- `.DS_Store`

### Real Log Producers Found

Within the audit boundary, only two `.log` classes were found:

1. Nautilus runtime file logs written at the repo root or worktree root
2. `tmp_tests/*.log` scratch files

Observed examples:

- `BOLT-001_2026-04-10_b6c3b939-882c-4e9d-86ae-4d42418e6f36.log`
- `.worktrees/fix-48-rust-worktree-enforcement/BOLT-001_2026-04-10_2573da64-b341-4d9a-a840-e0031e647236.log`
- `tmp_tests/issue-522.log`

### Runtime Log Naming Contract

`bolt-v2` itself only configures log levels via [`src/main.rs`](/Users/spson/Projects/Claude/bolt-v2/src/main.rs) and does not set a directory or file name.

Upstream Nautilus `FileWriter` defaults to:

- no directory when `FileWriterConfig.directory` is `None`
- no custom base name when `FileWriterConfig.file_name` is `None`
- file path shape:
  - `{trader_id}_{YYYY-MM-DD}_{instance_id}.log`

That behavior comes from Nautilus `crates/common/src/logging/writer.rs` and `crates/common/src/logging/config.rs`, where `LoggerConfig.fileout_level != Off` enables file output and the default file writer derives the basename from `trader_id`, UTC date, and instance id.

### Worktree Boundary

Official `git worktree`s are part of the supported operator flow for this repository and must be covered by the ignore policy. A rule that works only in the main checkout is not sufficient.

For issue `#72`, the implementation checkout itself is an attached worktree at `.worktrees/issue-72-log-ignore-policy`. Root-anchored ignore semantics can therefore be proven directly from this branch's checkout context without requiring a second branch or worktree to adopt the new `.gitignore` before merge.

### Tracked Fixture Requirement

There are currently no tracked `.log` fixtures in the repository, but the ignore policy must not prevent future intentional tracked fixtures such as:

- `tests/fixtures/example.log`

## Problem Statement

The current `*.log` rule is safe but too broad:

- it hides all `.log` files anywhere in the checkout
- it provides no documentation of which logs are actually expected
- it can mask future intentional `.log` fixtures

The replacement policy must stay behavior-preserving for operator/runtime logs while becoming explicit enough to describe the real artifact contract.

## Options Considered

### Option 1: Keep `*.log`

Pros:

- zero risk of exposing unknown logs
- no change to current operator flow

Cons:

- remains imprecise
- continues hiding all future `.log` files, including potential fixtures

### Option 2: Replace `*.log` with explicit root/worktree runtime and scratch patterns

Candidate rules:

- `/*_[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]_*.log`
- `/tmp_tests/*.log`

Pros:

- matches the actual default Nautilus runtime filename contract
- works at the root of the main checkout and at the root of each attached worktree
- allows future tracked fixtures outside those explicit paths

Cons:

- requires proof that non-sample trader ids still match
- requires proof that no other real `.log` producer in scope is missed

### Option 3: Narrow only to `/*.log`

Pros:

- simpler than Option 2
- still covers the root/worktree-root runtime logs observed so far

Cons:

- still broad
- does not document the actual runtime naming contract
- still hides any other root-level `.log` artifact regardless of meaning

## Decision

Choose **Option 2**.

The repository should replace `*.log` with explicit patterns that cover:

- default Nautilus runtime file logs at the checkout or worktree root
- scratch logs under `tmp_tests/`

This is the narrowest rule set that still matches the real in-scope artifact producers we found.

## Design

### `.gitignore`

Replace:

- `*.log`

With:

- `/*_[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]_*.log`
- `/tmp_tests/*.log`

Keep the other ignore rules unchanged.

### Scratch Cleanup

- if `tmp_tests/` exists in the current checkout, remove `tmp_tests/issue-522.log` if it is present
- if that leaves `tmp_tests/` empty, remove the directory
- if `tmp_tests/` is absent, scratch cleanup is a no-op

This cleanup is part of the branch only because `tmp_tests/*.log` is part of the explicit ignore decision.

### No Runtime Behavior Change

Do **not** change:

- [`src/main.rs`](/Users/spson/Projects/Claude/bolt-v2/src/main.rs)
- config schema
- runtime log directory behavior
- logger setup

The audit only narrows ignore behavior to match the already-observed logger contract.

## Proof Requirements

The implementation must prove all of the following:

1. A non-sample runtime log at the root of the current issue-72 attached worktree is ignored:
   - example: `ALPHA-999_2026-04-10_12345678-1234-1234-1234-123456789abc.log`
2. The proof is executed from an attached worktree context, demonstrating that the root-anchored rule works in official worktree usage for this branch.
3. A fixture path such as `tests/fixtures/example.log` is **not** ignored.
4. `tmp_tests/example.log` is ignored.
5. No broader behavior change is introduced outside `.gitignore` and scratch cleanup.

## Verification Plan

Required verification from the issue-72 attached worktree root:

- `git diff --name-only`
- `git diff --name-only origin/main...HEAD`
- `git check-ignore -v ALPHA-999_2026-04-10_12345678-1234-1234-1234-123456789abc.log`
- `git check-ignore -v tmp_tests/example.log`
- `! git check-ignore -v tests/fixtures/example.log`
- `CARGO_TARGET_DIR="$PWD/.target" ~/.cargo/bin/cargo test --no-run`

Expected:

- because this checkout is itself an attached worktree, the `git check-ignore` commands prove root-anchored behavior in a real worktree context
- `git diff --name-only` shows only intentional working-tree edits in this checkout
- `git diff --name-only origin/main...HEAD` stays limited to:
  - `.gitignore`
  - `docs/superpowers/specs/2026-04-10-log-ignore-policy-audit-design.md`
  - `docs/superpowers/plans/2026-04-10-log-ignore-policy-audit-implementation.md`
  - optional `tmp_tests/issue-522.log` or `tmp_tests/` cleanup only if present in this checkout

## Risks

### Missed Producer Risk

If another real `.log` producer exists in normal operator flow and does not match the explicit patterns, narrowing the ignore rule would surface previously ignored files. That is why the implementation must re-run the inventory and check-ignore proof before merge.

### Upstream Naming Drift

If Nautilus changes its default file naming contract later, the explicit rule could become stale. That is acceptable because the rule would then need to be revisited as part of a runtime/logging upgrade, not hidden by a blanket ignore.

## Success Criteria

This work is successful when:

- the broad `*.log` rule is removed
- explicit patterns cover real runtime and scratch logs for main checkout plus official worktrees
- future tracked fixture logs are not accidentally ignored
- no runtime behavior or operator path changes are introduced
