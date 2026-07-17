# Merge Queue Preflight Origin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make isolated merge-queue verifier worktrees resolve the same canonical `origin` URL used to fetch their pinned candidate commits.

**Architecture:** Keep the per-run private bare repository as the only mutable Git repository used by preflight. After `PrivateFetchRefs.fetch_origin()` normalizes the configured source, store that URL as `remote.origin.url` in the private repository so all linked verifier worktrees inherit it.

**Tech Stack:** Python 3 standard library, Git CLI, repository `just` verification recipes.

## Global Constraints

- Do not touch or write Git metadata in the user's source checkout.
- Do not weaken the dependency-direction verifier's fail-closed baseline check.
- Do not expose remote URLs or credentials in preflight output.
- Do not queue PR #1439 without its required code-owner approval.
- Use `just merge-queue <pr...>` as the only queue-entry mechanism.

---

### Task 1: Preserve `origin` in isolated verifier worktrees

**Files:**
- Modify: `scripts/test_merge_queue_preflight.py:1881`
- Modify: `scripts/test_merge_queue_preflight.py:5074`
- Modify: `scripts/merge_queue_preflight.py:1973`

**Interfaces:**
- Consumes: `PrivateFetchRefs.fetch_origin(origin: str) -> str` and the existing `GitFixture` remote.
- Produces: repository-local `remote.origin.url` configuration inherited by worktrees created in `run_verifier_commands()`.

- [ ] **Step 1: Write the failing regression test**

Add `assert_verifier_worktrees_inherit_origin_remote()` beside the existing verifier-worktree isolation tests. Create a fixture PR and a verifier script that runs `git remote get-url origin`, compares the resolved value with `fixture.remote`, and exits nonzero when the remote is absent or different. Run preflight with the strict verifier profile and assert an unblocked batch for PR 1. Register the assertion in the test runner's existing list.

- [ ] **Step 2: Run the targeted suite to verify RED**

Run:

```bash
python3 scripts/test_merge_queue_preflight.py
```

Expected: failure from the new assertion because preflight returns a `verifier_failed` block containing `No such remote 'origin'`.

- [ ] **Step 3: Write the minimal implementation**

Refactor the existing success/fallback branches in `PrivateFetchRefs.fetch_origin()` to converge before returning, then configure the private repository:

```python
if result.returncode != 0 or not remote_url:
    remote_url = fetchable_origin_argument(origin, self.source_repo)
else:
    remote_url = fetchable_remote_url(remote_url, self.source_repo)
git(
    self.git_repo,
    "config",
    "--local",
    "remote.origin.url",
    remote_url,
    timeout_seconds=self.input_timeout_seconds,
)
self.remotes[origin] = remote_url
return remote_url
```

- [ ] **Step 4: Run the targeted suite to verify GREEN**

Run:

```bash
python3 scripts/test_merge_queue_preflight.py
```

Expected: `OK: merge_queue_preflight tests passed.`

- [ ] **Step 5: Run changed-script syntax and diff checks**

Run:

```bash
python3 -m py_compile scripts/merge_queue_preflight.py scripts/test_merge_queue_preflight.py
git diff --check
```

Expected: both commands exit 0 with no diagnostics.

- [ ] **Step 6: Commit the test and implementation**

```bash
git add scripts/merge_queue_preflight.py scripts/test_merge_queue_preflight.py
git commit -m "fix: preserve origin in preflight worktrees"
```

### Task 2: Verify and publish the governed fix

**Files:**
- Verify: `docs/ci/merge-queue-preflight-contract.md`
- Verify: `ci/rust-verification.toml`
- Verify: `scripts/merge_queue_preflight.py`
- Verify: `scripts/test_merge_queue_preflight.py`

**Interfaces:**
- Consumes: the Task 1 fix branch and repository verification recipes.
- Produces: exact-head local evidence and a reviewed GitHub pull request.

- [ ] **Step 1: Run permitted local repository gates**

Run:

```bash
just fmt-check
just ci-lint-workflow
just source-fence-static
```

Expected: all commands exit 0. Do not run local compile-heavy Rust verification.

- [ ] **Step 2: Generate the implementation-branch audit checklist**

Inspect the implementation diff for every added `if`, `match`, `except`, `unwrap_or`, `unwrap_or_default`, `or_else`, and default branch. Record why each branch is validation or classification rather than silent fallback in the PR evidence; the intended implementation adds no new conditional branch.

- [ ] **Step 3: Perform an internal adversarial review**

Check that the fix neither mutates the source checkout nor exposes the remote URL, preserves temporary-repository cleanup, supports raw path/URL `--origin` inputs after normalization, and does not weaken verifier failure classification.

- [ ] **Step 4: Publish a draft pull request**

Use `just sandbox-safe-push`, open a draft PR whose lasting body names only this preflight-origin scope, and record the pushed head SHA outside the PR body.

- [ ] **Step 5: Obtain exact-head evidence and required review**

Before the governed Task 7 cutover, mark the coherent PR ready and run `just verify-remote`; confirm required checks on the exact head. Request review from the login currently resolving GitHub node ID `U_kgDOEZMFhA`. Resolve all findings and obtain approval at the final head.

- [ ] **Step 6: Merge through native controls**

Verify active `main` rules, exact-head checks, required approval, last-push approval, stale-review dismissal, and resolved threads. Merge without `--admin` and confirm `origin/main` contains the fix.

### Task 3: Queue the eligible pull-request wave

**Files:**
- Verify: `scripts/merge_queue_preflight.py`
- Verify: `justfile`

**Interfaces:**
- Consumes: a fresh clean worktree at fixed `origin/main` and exact live PR heads.
- Produces: a Mergify queue request posted only by `just merge-queue`.

- [ ] **Step 1: Re-verify live PR state**

Confirm #1452, #1448, and #1449 remain open, mergeable, clean, approved, and unqueued. Confirm #1439 remains excluded unless its required code-owner approval has landed.

- [ ] **Step 2: Run direct JSON preflight at exact SHAs**

Resolve live base and PR-head refs, run `scripts/merge_queue_preflight.py` with all expected SHA arguments, and require `verdict == "queue_as_one_wave"` with one batch containing `[1452, 1448, 1449]`.

- [ ] **Step 3: Queue through the repository recipe**

Run:

```bash
just merge-queue 1452 1448 1449
```

Expected: the recipe repeats preflight successfully and posts the configured Mergify queue command once.

- [ ] **Step 4: Verify queue state and clean up**

Report the exact base/head SHAs and live Mergify queue state. Remove the temporary clean worktree only after no further governed fix or queue verification is required; leave the dirty primary worktree untouched.
