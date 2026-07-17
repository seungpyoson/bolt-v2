# Merge Queue Preflight Origin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make isolated merge-queue verifier worktrees resolve the same canonical `origin` URL used to fetch their pinned candidate commits.

**Architecture:** Keep the per-run private bare repository and linked verifier worktrees as the only mutable Git repositories used by preflight. The operator must snapshot TOML once, resolve exactly one configured Git remote name before `ls-remote`, use fully qualified refs, and pass the same config snapshot to preflight. `PrivateFetchRefs.fetch_origin()` must resolve that configured name; missing remotes fail inconclusively and raw URL/path inputs are never reinterpreted as an alternate source. Normalize and validate the remote's credential-free URL, pass it into verifier execution, and store it only as worktree-specific `remote.origin.url` configuration; the private bare repository remains remote-free. Redact the normalized URL from Git failures, verifier streams, progress diagnostics, and public verifier-command JSON.

**Tech Stack:** Python 3 standard library, Git CLI, repository `just` verification recipes.

## Global Constraints

- Do not touch or write Git metadata in the user's source checkout.
- Do not weaken the dependency-direction verifier's fail-closed baseline check.
- Do not expose remote URLs or credentials in preflight output.
- Do not substitute a URL, path, default branch, mutable config reread, or alternate remote after any validation failure.
- Use `just merge-queue <pr...>` as the only queue-entry mechanism.

---

### Task 1: Preserve `origin` in isolated verifier worktrees

**Files:**
- Modify: `scripts/test_merge_queue_preflight.py:1881`
- Modify: `scripts/test_merge_queue_preflight.py:5074`
- Modify: `scripts/merge_queue_preflight.py:1973`

**Interfaces:**
- Consumes: `PrivateFetchRefs.fetch_origin(origin: str) -> str`, where `origin` is one configured Git remote name, and the existing `GitFixture` remote.
- Produces: worktree-local `remote.origin.url` configuration created and removed with each worktree in `run_verifier_commands()`.

- [x] **Step 1: Write the failing regression test**

Add `assert_verifier_worktrees_inherit_origin_remote()` beside the existing verifier-worktree isolation tests. Create a fixture PR whose configured `origin` remote contains `../origin.git` and a verifier script that runs `git remote get-url origin`, compares the normalized value with `fixture.remote`, proves `git config --worktree` owns it, and proves `git config --local` does not. Add negative tests that reject raw path-like origin arguments plus password, query-token, fragment-token, and non-SSH-userinfo URLs before Git fetch/config while preserving credential-free SSH/SCP usernames. Redact the normalized URL from configuration errors, failed verifier streams, progress diagnostics, and public verifier-command JSON. Run preflight with the strict verifier profile and assert an unblocked batch for PR 1. Register the assertions in the test runner's existing list.

- [x] **Step 2: Run the targeted suite to verify RED**

Run:

```bash
python3 scripts/test_merge_queue_preflight.py
```

Expected: failure from the new assertion because preflight returns a `verifier_failed` block containing `No such remote 'origin'`.

- [x] **Step 3: Write the minimal implementation**

Pass the normalized origin URL through the existing verifier batching calls. In `run_verifier_commands()`, enable worktree-specific configuration before adding the worktree, then configure the remote inside the worktree's existing cleanup boundary:

```python
git(
    repo,
    "config",
    "extensions.worktreeConfig",
    "true",
    timeout_seconds=input_timeout_seconds,
)
git(
    worktree,
    "config",
    "--worktree",
    "remote.origin.url",
    origin_url,
    timeout_seconds=input_timeout_seconds,
)
```

Before Git fetch or config, reject any parsed password, URL query or fragment
component, and userinfo on non-SSH schemes while preserving credential-free
standard SSH/SCP usernames. Do not retry or switch origin sources. Run worktree
configuration with `check=False`, convert failure to a redacted `PreflightError`,
and replace the normalized URL in captured verifier streams, progress
diagnostics, and public verifier-command JSON with `<remote-url>`.

- [x] **Step 4: Run the targeted suite to verify GREEN**

Run:

```bash
python3 scripts/test_merge_queue_preflight.py
```

Expected: `OK: merge_queue_preflight tests passed.`

- [x] **Step 5: Run changed-script syntax and diff checks**

Run:

```bash
python3 -m py_compile scripts/merge_queue_preflight.py scripts/test_merge_queue_preflight.py
git diff --check
```

Expected: both commands exit 0 with no diagnostics.

- [x] **Step 6: Commit the test and implementation**

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

- [x] **Step 0: Close operator authority and ref-selection alternate paths**

Add failing tests for raw URL/path origin config, unsafe resolved URLs, config
mutation between SHA resolution and preflight, and unqualified base fetches.
Snapshot TOML bytes once, resolve only the configured remote name, validate its
one resolved URL before `ls-remote`, pass the snapshot to preflight, and fetch
the base only as `refs/heads/<base>`. Any error terminates without another
origin, config, URL, path, ref, or default branch.

- [x] **Step 0a: Bind remote identity across preflight and queueing**

Pass only the resolved URL's opaque SHA-256 identity to preflight, require its
independent configured-name resolution to match before any fetch, and derive
both preflight `GH_REPO` and the explicit queue `gh --repo` value from that same
URL. Reject non-GitHub URLs before preflight. Tests prove config mutation
terminates rather than selecting a second preflight origin or implicit queue
repository.

- [ ] **Step 1: Run permitted local repository gates**

Run:

```bash
just fmt-check
just ci-lint-workflow
just source-fence-static
```

Expected: all commands exit 0. Do not run local compile-heavy Rust verification.

- [ ] **Step 2: Generate the implementation-branch audit checklist**

Inspect the implementation diff for every added `if`, `match`, `except`, `unwrap_or`, `unwrap_or_default`, `or_else`, and default branch. Record why each branch terminates on invalid input or classifies evidence rather than selecting a substitute source, secret, verifier, or fetch path.

- [ ] **Step 3: Perform an internal adversarial review**

Check that the fix neither mutates the source checkout nor exposes the remote URL, preserves temporary-repository cleanup, keeps origin and base solely TOML-authoritative with no CLI override, and does not weaken verifier failure classification.

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
