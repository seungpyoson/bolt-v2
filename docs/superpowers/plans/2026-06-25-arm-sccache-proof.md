# ARM Sccache Proof Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a manual, non-gating CI proof lane that measures whether S3-backed sccache can make ARM root nextest archive builds fast enough to keep ARM as the long-term CI target.

**Architecture:** Keep ARM runners and the managed Rust launcher as the only compile path. Introduce a TOML-governed, CI-only compiler-wrapper exception so `rust_verification.py` injects `RUSTC_WRAPPER=sccache` without allowing raw workflow-level wrapper overrides. Add one manual `sccache_proof` CI job that builds the root nextest archive on ARM and reports sccache stats.

**Tech Stack:** GitHub Actions, Ubicloud ARM64 runner, `scripts/rust_verification.py`, `ci/rust-verification.toml`, `sccache` S3 backend, OIDC AWS role.

## Global Constraints

- Keep `CI_RUNNER_MANAGED_HEAVY=ubicloud-standard-4-arm`; do not flip runner architecture.
- Do not run local compile-heavy Rust verification; use repo-approved local static checks only.
- Do not set `RUSTC_WRAPPER` directly in workflow YAML.
- Do not reuse the deploy AWS role for compile cache.
- The proof lane is manual-only, non-gating, and must not alter merge proof semantics.
- Evidence class: workflow/policy changes require targeted static checks plus internal structural review; performance claim requires remote CI run logs with sccache stats.

---

### Task 1: Managed Sccache Opt-In

**Files:**
- Modify: `ci/rust-verification.toml`
- Modify: `scripts/rust_verification.py`
- Modify: `scripts/test_rust_verification_cache_retention.py`

**Interfaces:**
- Consumes: ambient `BOLT_RUST_VERIFICATION_SCCACHE=1`, `GITHUB_ACTIONS=true`, optional `SCCACHE_PATH`.
- Produces: managed cargo child env with `RUSTC_WRAPPER` set only to the configured sccache wrapper path/name.

- [ ] Add `[remote_compile_cache]` to `ci/rust-verification.toml` with `enabled`, `enable_env`, `ci_env`, `wrapper_env`, and `wrapper_program`.
- [ ] Update `managed_env()` so ambient wrapper env is still scrubbed first.
- [ ] If the opt-in env is set in GitHub Actions, inject only the configured wrapper, validating its basename is `sccache`.
- [ ] Add tests proving ambient `RUSTC_WRAPPER` is scrubbed by default and the CI-only opt-in injects the configured wrapper rather than a leaked value.
- [ ] Evidence: `python3 scripts/test_rust_verification_cache_retention.py`.

### Task 2: Manual Root Archive Proof Job

**Files:**
- Modify: `.github/workflows/ci.yml`
- Modify: `ci/github-actions-runners.toml`
- Modify: `.github/actionlint.yaml`

**Interfaces:**
- Consumes repo variables `AWS_CI_CACHE_ROLE_ARN`, `CI_SCCACHE_BUCKET`, `CI_SCCACHE_REGION`, and `CI_SCCACHE_S3_KEY_PREFIX`.
- Produces a manual `sccache-proof` job with archive build time and `sccache --show-stats` output.

- [ ] Add workflow_dispatch input `sccache_proof` defaulting to `false`.
- [ ] Add `sccache-proof` job gated on `workflow_dispatch && inputs.sccache_proof == 'true'`.
- [ ] Give the proof job `id-token: write`, configure the separate CI cache role, install sccache, zero stats, run `just test-archive`, and print stats.
- [ ] Map `sccache-proof` to `managed_heavy` in `ci/github-actions-runners.toml`.
- [ ] Allow the four new repo variables in `.github/actionlint.yaml`.
- [ ] Evidence: `just ci-lint-workflow` and `python3 scripts/test_verify_ci_workflow_hygiene.py`.

### Task 3: Remote Measurement

**Files:**
- No additional repo files.

**Interfaces:**
- Consumes the branch pushed to GitHub and repo variables configured for the proof role/bucket/prefix.
- Produces two dispatch results: cold-ish first proof and warm second proof on the same ref.

- [ ] Push branch and open a draft PR.
- [ ] Confirm the repo variables exist before dispatching.
- [ ] Dispatch `CI` with `sccache_proof=true` and `full_ci=false` twice on the same ref.
- [ ] Record archive build duration and sccache hit/miss stats from both runs.
- [ ] Evidence: GitHub Actions run URLs and log excerpts.
