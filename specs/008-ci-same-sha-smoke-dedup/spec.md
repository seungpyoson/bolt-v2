# Feature Specification: #205 Same-SHA Smoke-Tag Dedup

**Feature Branch**: `codex/ci-205-same-sha-smoke-dedup`
**Created**: 2026-05-15
**Status**: Draft
**Input**: GitHub issue #205 plus epic #333 comments as live acceptance source

## Scope

This slice owns #205 only: post-merge tag deploys must reuse exact same-SHA `main` CI evidence instead of rerunning equivalent heavy lanes. It does not change PR critical-path decomposition, nextest cache preservation, residual path-filter docs/pass-stubs, or neutral config path relocation.

## User Stories & Tests

### User Story 1 - Reuse trusted same-SHA main evidence (P1)

As the maintainer, I can push a smoke tag on a commit that already passed `main` CI and reach deploy without rerunning equivalent `fmt-check`, `deny`, `clippy`, `check-aarch64`, `source-fence`, `test`, or `build` work.

**Independent Test**: A workflow verifier fixture fails unless tag runs have a `same-sha-main-evidence` job, skip duplicate heavy lanes, require that evidence in `gate`, and download the main-run artifact by artifact ID.

**Acceptance Scenarios**:

1. **Given** a tag points at a SHA with a completed successful `main` push CI run, **When** the tag workflow starts, **Then** it resolves that exact CI run by SHA, workflow path, event, branch, status, and conclusion.
2. **Given** the trusted `main` run uploaded `bolt-v2-binary`, **When** deploy downloads the artifact, **Then** it downloads by artifact ID from the source run, not by rebuilding or inferring from branch state.
3. **Given** tag reuse is active, **When** the aggregate gate runs, **Then** duplicate heavy jobs are expected skipped and evidence success is required.

### User Story 2 - Fail closed on unsafe reuse (P1)

As the maintainer, I get a failed tag workflow instead of an implicit rebuild or deploy if exact evidence is missing, stale, incomplete, cancelled, skipped unexpectedly, or failed.

**Independent Test**: `scripts/test_find_same_sha_main_evidence.py` rejects wrong branch, wrong SHA, wrong workflow path, skipped required job, missing test shard, expired artifact, and artifact SHA mismatch payloads.

**Acceptance Scenarios**:

1. **Given** no successful `main` CI run exists for the tag SHA, **When** evidence resolution runs, **Then** it exits non-zero and deploy remains blocked.
2. **Given** a required source-run lane is skipped, cancelled, failed, or absent, **When** evidence resolution validates jobs, **Then** it exits non-zero with the lane name.
3. **Given** the artifact is expired or bound to a different run/SHA/branch, **When** evidence resolution validates artifacts, **Then** it exits non-zero and no deploy artifact is consumed.

### User Story 3 - Preserve PR and main CI safety (P1)

As the maintainer, I can keep PR CI and `main` push CI as full verification surfaces while changing only the immediate smoke-tag topology.

**Independent Test**: `just ci-lint-workflow` fails if non-tag lanes stop running normally, if deploy loses direct safety needs, or if the tag reuse path bypasses the aggregate `gate`.

**Acceptance Scenarios**:

1. **Given** a pull request run, **When** the workflow evaluates jobs, **Then** existing required lanes still run and `same-sha-main-evidence` is skipped.
2. **Given** a `main` push run, **When** the workflow evaluates jobs, **Then** the build artifact is produced by the normal build job and later tag runs can reuse it.
3. **Given** deploy starts, **When** logs are inspected, **Then** the source run ID, check suite ID, artifact ID, and source SHA are printed.

### User Story 4 - Record before/after proof boundary (P2)

As the maintainer, I can distinguish implementation readiness from issue closure: the PR can ship the fail-closed reuse path, but #205 closes only after real smoke-tag evidence proves duplicate `test`/`build` time is reduced.

**Independent Test**: The PR/issue handoff names the historical before run IDs and records the missing after-evidence blocker until the stack lands and a real smoke tag runs.

## Requirements

- **FR-001**: Same-SHA reuse MUST match the tag SHA exactly against a completed successful `main` push CI run.
- **FR-002**: The matched source run MUST be workflow `CI` at `.github/workflows/ci.yml`, event `push`, branch `main`, status `completed`, conclusion `success`, and not the current tag run.
- **FR-003**: Reused source evidence MUST include successful required jobs for the current topology: `detector`, `fmt-check`, `deny`, `clippy`, `check-aarch64`, `source-fence`, four `test` shards, `build`, and `gate`.
- **FR-004**: Evidence resolution MUST fail closed when the source run is absent, stale, incomplete, cancelled, skipped unexpectedly, failed, malformed, or for a different SHA.
- **FR-005**: Artifact reuse MUST bind to the exact source run and SHA by artifact ID, with artifact name `bolt-v2-binary`, non-expired state, workflow run ID, branch `main`, and matching head SHA.
- **FR-006**: Tag runs MUST skip duplicate heavy lanes instead of falling back to rerun them.
- **FR-007**: `gate` MUST have explicit tag and non-tag modes: tag mode requires evidence success and duplicate-lane skips; non-tag mode requires evidence skipped and existing lane success semantics.
- **FR-008**: `deploy` MUST retain direct safety needs plus `gate` and `same-sha-main-evidence`, run only for tags after both succeed, and use `always()` so expected skipped lanes do not suppress evaluation.
- **FR-009**: `deploy` MUST log source run ID, check suite ID, artifact ID, and source SHA before artifact use.
- **FR-010**: PR CI MUST NOT be weakened; this issue applies only to post-merge `main`/tag topology.
- **FR-011**: The workflow verifier and resolver self-tests MUST cover both positive selection and negative fail-closed cases.
- **FR-012**: The implementation MUST NOT claim #205 complete until after a real post-merge smoke-tag run proves reduced duplicate `test`/`build` time.

## Key Entities

- **SameShaMainEvidence**: Source CI run ID, check suite ID, artifact ID, artifact size, artifact name, source SHA, and source URL.
- **TrustedSourceRun**: A completed successful `main` push `CI` run at `.github/workflows/ci.yml` for the exact tag SHA.
- **ReusableArtifact**: The `bolt-v2-binary` artifact uploaded by the trusted source run and bound to the same run ID, branch, and SHA.
- **TagReuseGate**: The aggregate gate mode that requires evidence success and verifies duplicate lanes were skipped on tag runs.

## Edge Cases

- Multiple workflow runs can exist for the same SHA; select only a run that passes all source-run, job, and artifact checks.
- Historical #205 before evidence predates the current four-shard topology and cannot satisfy current reuse requirements.
- A tag on a SHA not yet green on `main` must fail instead of rerunning heavy jobs.
- A source run with an expired artifact must fail even if jobs passed.
- A docs-only PR or PR path-filter behavior is not part of this issue.

## Success Criteria

- **SC-001**: `python3 -B scripts/test_find_same_sha_main_evidence.py` passes.
- **SC-002**: `python3 -B scripts/test_verify_ci_workflow_hygiene.py` passes.
- **SC-003**: `python3 -B scripts/verify_ci_workflow_hygiene.py` passes against `.github/workflows/ci.yml`.
- **SC-004**: `just ci-lint-workflow` passes.
- **SC-005**: The PR records that real after evidence is pending until the stack can run a post-merge smoke tag.
