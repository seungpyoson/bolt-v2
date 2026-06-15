# Verification Feedback Lanes Plan

Date: 2026-06-15

## Problem Map

The current verification workflow has four separate problems. They must stay separate because they have different root causes and fixes.

Tracking:

- Epic: #333, CI wall-time and topology cleanup.
- Problem 1: #739, local source-fence verifier lanes serialize independent checks.
- Problem 2: #740, add a lane-aware scheduler for local verification commands.
- Problem 3: #741, harden Rust Probe into a safe agent-facing remote debug lane.
- Problem 4: #742, clarify agent-facing verification lane routing.

### Problem 1: Local verifier gates are over-serialized

Symptom:

```text
local lane lock is currently held by another verifier
verify_bolt_v3_runtime_literals.py
checks are queued rather than running concurrently
```

Root cause: every governed `scripts/verify_*.py` and `scripts/test_*.py` entry point acquires one repo-wide machine lock through `[local_lane_policy]`. `just source-fence-static` runs many of those governed scripts in sequence, so separate agent sessions can queue behind unrelated local verifier work.

This is not a Rust compile problem. Rust Probe does not fix this.

### Problem 2: Agents have no lane-aware local verification scheduler

Agents can launch overlapping local checks such as `just fmt-check`, `just source-fence-static`, and `just ci-lint-workflow` at the same time. The repo only serializes them after they start competing for the same lane lock.

Root cause: the repo has a lane lock, but no top-level scheduler that turns overlapping local verification requests into one ordered, deduplicated plan.

### Problem 3: Rust compile/test feedback lacks a safe agent-facing remote command

The repo already has the manual Rust Probe primitive:

- `.github/workflows/rust-probe.yml`
- `.github/scripts/run-rust-probe.sh`
- `scripts/test_run_rust_probe.py`

But it is not yet a safe agent-facing lane:

- no `just rust-probe` recipe;
- no `scripts/rust_verification.py rust-probe` subcommand;
- probe workflow concurrency is still global;
- `ref` defaults to `main`;
- the runner does not assert the checked-out SHA;
- there is no wrapper-owned run correlation, active-run cap, timeout policy, or refusal-text integration.

### Problem 4: Agent-facing verification lane routing is unclear

Agents do not have a clear routing decision tree for verification symptoms. Local compile-heavy Rust refusals point directly toward `just verify-remote`, while local lane-lock symptoms and Rust compile/test debugging need different middle lanes. This makes agents confuse local Python/source-fence queues with Rust compile/test feedback, launch competing local gates, or use full CI as a discovery mechanism.

Expected routing:

- local source-fence/Python verifier queueing -> local verifier scheduler work;
- targeted Rust compile/test debugging -> hardened Rust Probe wrapper;
- merge-proof evidence -> `just verify-remote`.

## Scope Of This Plan

This plan fixes Problem 3 only: turn the existing manual Rust Probe into a safe remote Rust debugging command for agents.

It does not fix Problem 1 or Problem 2. Those require a separate local verifier orchestrator plan. It enables part of the eventual Problem 4 routing fix by creating the Rust debug lane that guidance can safely point to.

## Invariants

- Keep local `cargo test`, `cargo clippy`, and `cargo build` blocked by policy.
- Keep managed local `just test`, `just clippy`, and `just build` refusing locally.
- Keep `just verify-remote` as the only merge-proof path.
- Keep Rust Probe debugging-only. A green probe must never satisfy `gate`, `verify-remote`, branch protection, or merge readiness.
- Do not create a second Rust feedback workflow for v0.

## Reviewed Rust Probe v0 Design

### Reuse The Existing Workflow

Reuse `.github/workflows/rust-probe.yml` for v0. Creating `rust-feedback.yml` would duplicate the Rust debugging primitive, runner mapping, metering, and governance surface.

### Add A Managed Wrapper

Add a `rust-probe` subcommand to `scripts/rust_verification.py`, exposed by:

```bash
just rust-probe <mode> [test_target] [test_name]
```

The wrapper must:

- refuse dirty worktrees, including untracked files;
- require a named branch with upstream;
- require local `HEAD` equals the upstream branch tip;
- dispatch `gh workflow run rust-probe.yml --ref <current-branch>`;
- pass exact source as workflow input `ref=<HEAD_SHA>`;
- pass a separate `expected_sha=<HEAD_SHA>` input for runner assertion;
- pass the exact declared workflow input keys, not aliases;
- generate and pass a unique `probe_id`;
- poll the matching run with an appearance deadline;
- print run URL, exact SHA, selected scope, and `NOT MERGE PROOF -- run just verify-remote for proof`.

Do not reuse the full-CI `verify-remote` poller directly. Its proof loop is PR-coupled and CI-workflow-specific.

### Use Branch-Scoped Concurrency For v0

Use branch/worktree as the v0 isolation unit:

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true
```

This gives parallel probes across branches and cancels stale probes on the same branch. It intentionally does not support multiple independent probes on the same branch at the same time. That is acceptable for v0 because agent work should be branch/worktree isolated.

If same-branch parallelism becomes a real requirement, add session-level concurrency later with required `session_id`, required `probe_id`, run-name correlation, and stronger global cost controls.

### Correlate Runs By Probe Id

Add a top-level workflow `run-name` that includes the wrapper-generated `probe_id`. The wrapper should poll by `displayTitle`/run name and then track the concrete run id.

Do not rely on `headSha` alone. For `workflow_dispatch`, the run's `headSha` describes the dispatch branch tip, while the probe source is checked out from `inputs.ref`.

Handle cancelled runs distinctly. A cancelled probe caused by `cancel-in-progress` is superseded, not a code failure.

### Fail Closed On Source Identity

Remove the `ref: main` default from `rust-probe.yml`. A missing `ref` must not test `main`.

After checkout, `.github/scripts/run-rust-probe.sh` must:

- validate `expected_sha` is non-empty and a full SHA;
- run `git rev-parse HEAD`;
- fail if the actual checkout SHA differs from `expected_sha`;
- print the actual SHA and probe scope for auditability.

Use `fetch-depth: 0` or an equivalent fetch strategy so an exact SHA checkout is reliable.

### Configuration

Add `[remote_probe]` to `ci/rust-verification.toml` for probe runtime policy:

- poll interval;
- appearance timeout;
- overall timeout;
- active-run caps;
- per-mode runner tier defaults;
- allowed runner tiers;
- workflow job timeout values.

Update policy validation for the new table. Runtime values must not be hardcoded in Python.

Keep runner mapping and metering in `ci/github-actions-runners.toml`. If dispatch metadata is added there, validate it consistently with the existing runner workflow mapping.

### Cost Controls

v0 cost controls:

- per-branch cancellation through workflow concurrency;
- wrapper active-run cap from `[remote_probe]`;
- workflow `timeout-minutes`;
- wrapper polling timeout;
- mode allowlist only;
- no arbitrary cargo args;
- no cache writes in v0;
- measure cold Rust Probe latency before advertising the command in refusal text.

The active-run cap is a wrapper-enforced refusal, not a perfect global lock. The hard global ceiling remains the GitHub/Ubicloud runner limit.

### Governance Tests

Add tests before updating refusal text:

- Rust Probe remains `workflow_dispatch` only;
- Rust Probe is not in `ci_provenance.full_ci.required_jobs`;
- Rust Probe is not in `gate.needs`;
- concurrency is no longer a global constant;
- `timeout-minutes` exists for probe jobs;
- workflow input keys used by the wrapper match declared YAML inputs;
- dirty worktree, untracked worktree, missing upstream, and `HEAD != upstream` refusals are tested;
- missing/empty `ref` fails closed;
- checked-out SHA assertion is tested;
- cancelled probe runs are not reported as code failures;
- `[remote_probe]` policy validation is tested;
- local cargo blocking and `just test` / `just clippy` / `just build` behavior stay unchanged.

## Implementation Order

1. Add `[remote_probe]` policy and validation.
2. Harden `rust-probe.yml`: branch-scoped concurrency, `run-name`, `probe_id`, `expected_sha`, no `main` ref default, timeouts, reliable checkout.
3. Harden `run-rust-probe.sh`: validate identity inputs and assert checked-out SHA.
4. Add `scripts/rust_verification.py rust-probe` with PR-free preconditions, dispatch, run correlation, active-run cap, and probe-specific state handling.
5. Add `just rust-probe`.
6. Add governance and wrapper tests.
7. Measure cold probe latency.
8. Update `AGENTS.md` wording without weakening `Rust Probe is debugging, not proof`.
9. Update local compile refusal text to mention `just rust-probe` only after the command is safe.

## Explicit Non-Goals

- Do not remote-back `just test`, `just clippy`, or `just build`.
- Do not remove local cargo blocking.
- Do not require an open PR for Rust Probe.
- Do not add pull request or push triggers to `rust-probe.yml`.
- Do not add Rust Probe to required jobs, `gate.needs`, or branch protection.
- Do not treat Rust Probe success as merge proof.
- Do not add read/write caches in v0.
- Do not claim this fixes local Python verifier lane queues.
