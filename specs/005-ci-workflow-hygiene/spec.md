# Feature Specification: CI Workflow Hygiene

**Feature Branch**: `codex/ci-203-workflow-hygiene`
**Created**: 2026-05-15
**Status**: Draft
**Input**: User description: "Address #203 from epic #333 after #342, without narrowing the issue body."

## User Scenarios & Testing

### User Story 1 - Workflow Topology Lint Is Explicit (Priority: P1)

As the maintainer, I can run one repo-local lint command and get actionable failures when a required CI job, dependency, gate result check, or deploy safety edge is missing.

**Why this priority**: #203 owns workflow correctness and defense-in-depth after topology-changing issues. The current #342 base has narrow source-fence checks, but not one exact-job topology contract for every required lane.

**Independent Test**: A self-test mutates representative CI YAML fixtures and proves the verifier fails for missing exact jobs, missing gate needs, missing result checks, and missing deploy direct needs.

**Acceptance Scenarios**:

1. **Given** a required job such as `source-fence` is absent, **When** the CI workflow hygiene verifier runs, **Then** it fails with the missing job id.
2. **Given** `gate` omits a required lane from `needs`, **When** the verifier runs, **Then** it fails with the exact missing lane name.
3. **Given** `deploy` can only see `gate` and `build`, **When** the verifier runs, **Then** it fails because direct defense-in-depth needs for required safety lanes are absent.

### User Story 2 - Setup Work Is Lane-Specific (Priority: P1)

As the maintainer, I can keep the shared setup action while avoiding managed target-dir resolution in jobs that do not use the managed target cache.

**Why this priority**: #203 explicitly calls out setup overreach. On the #342 base, `fmt-check` and `deny` use the managed owner, but they do not use `steps.setup.outputs.managed_target_dir`.

**Independent Test**: The verifier fails unless target-cache jobs opt into managed target-dir resolution and non-target-cache jobs do not.

**Acceptance Scenarios**:

1. **Given** a job uses `steps.setup.outputs.managed_target_dir`, **When** setup is configured, **Then** it explicitly opts into target-dir resolution.
2. **Given** `fmt-check` or `deny`, **When** setup is configured, **Then** those jobs do not opt into managed target-dir resolution.
3. **Given** a setup action invocation lacks `just-version` or token wiring, **When** the existing workflow lint runs, **Then** it still fails with the existing managed setup checks.

### User Story 3 - Detector And Deploy Semantics Are Preserved (Priority: P1)

As the maintainer, I can reduce unnecessary serialization without weakening the aggregate gate, deploy safety, or build detector semantics.

**Why this priority**: #203 asks to re-evaluate `fmt-check needs: detector` and deploy direct needs. The root solution is to remove unnecessary `fmt-check` serialization while adding direct deploy needs as defense-in-depth.

**Independent Test**: `just ci-lint-workflow` passes only when `fmt-check` has no detector dependency, `build` remains detector-gated, and `deploy` directly needs gate, build, detector, fmt-check, deny, clippy, source-fence, and test.

**Acceptance Scenarios**:

1. **Given** `fmt-check` does not consume detector output, **When** the workflow runs, **Then** `fmt-check` can start without waiting for detector.
2. **Given** `build` consumes `needs.detector.outputs.build_required`, **When** the workflow runs, **Then** `build` still depends on detector and remains skipped only through detector output.
3. **Given** a tag deploy run, **When** deploy starts, **Then** deploy has direct needs on the required safety lanes plus the aggregate gate and build artifact lane.

## Edge Cases

- #342 has landed in the stacked base, so source-fence topology is an active #203 invariant.
- #332 has not landed in this base. The verifier must not invent test shard or `check-aarch64` top-level requirements before that topology exists.
- #205 has not landed in this base. Same-SHA deploy reuse lint is out of this slice until a reuse path exists.
- #344 pass-stub behavior has not landed in this base. Required-check stub drift lint is out of this slice until a stub workflow exists.
- `fmt-check` still needs the managed Rust owner for `just fmt-check`; only managed target-dir resolution is trimmed.
- The verifier must parse enough GitHub Actions YAML shape for this repo without adding unpinned dependencies.

## Requirements

### Functional Requirements

- **FR-001**: The repository MUST have a deterministic CI workflow hygiene verifier that uses only standard-library tooling.
- **FR-002**: `just ci-lint-workflow` MUST run the CI workflow hygiene verifier self-tests and the verifier.
- **FR-003**: The verifier MUST require exact current CI job ids: `detector`, `fmt-check`, `deny`, `clippy`, `source-fence`, `test`, `build`, `gate`, and `deploy`.
- **FR-004**: The verifier MUST require `gate.needs` and gate result checks for detector, fmt-check, deny, clippy, source-fence, test, and build.
- **FR-005**: The verifier MUST require `source-fence` to depend on detector and `test` to depend on source-fence while #342 owns the early-fail lane.
- **FR-006**: The verifier MUST require `build` to depend on detector and to gate on `needs.detector.outputs.build_required`.
- **FR-007**: The verifier MUST require `deploy.needs` to include gate, build, detector, fmt-check, deny, clippy, source-fence, and test.
- **FR-008**: The workflow MUST remove the unnecessary `fmt-check` dependency on detector.
- **FR-009**: The shared setup action MUST make managed target-dir resolution opt-in.
- **FR-010**: Jobs using `steps.setup.outputs.managed_target_dir` MUST opt into managed target-dir resolution; jobs not using that output MUST NOT opt in.
- **FR-011**: The verifier MUST print actionable errors naming the missing or wrong job, dependency, gate check, or setup opt-in.
- **FR-012**: The branch MUST not implement #332 sharding, #195 cache retention, #205 same-SHA deploy reuse, #335 path filters, #344 pass-stub/evidence work, or #340 config relocation.
- **FR-013**: Exact-head CI evidence MUST show `detector`, `fmt-check`, `deny`, `clippy`, `source-fence`, `test`, `build`, and `gate` passing on the final PR head.
- **FR-014**: The verifier MUST require CI build-tool installs to avoid source-building `cargo-deny`, `cargo-nextest`, and `cargo-zigbuild`: `cargo-deny` and `cargo-nextest` use pinned `taiki-e/install-action` with `fallback: none`, and `cargo-zigbuild` uses a checksum-verified prebuilt release archive.

### Key Entities

- **WorkflowTopologyContract**: Required current GitHub Actions jobs, needs edges, and gate result checks.
- **WorkflowHygieneVerifier**: Standard-library script that validates the topology contract and setup opt-ins.
- **SetupTargetDirOptIn**: Shared setup action input controlling managed target-dir resolution.
- **DeployDefenseNeeds**: Direct deploy dependencies on all required safety lanes, not only the aggregate gate.
- **DetectorSerializationDecision**: Evidence-backed decision that only build remains detector-output-gated while fmt-check can run independently.
- **PrebuiltToolInstallContract**: CI policy that keeps tool versions sourced from setup outputs while preventing slow `cargo install` source builds in required PR lanes.

## Success Criteria

### Measurable Outcomes

- **SC-001**: The workflow hygiene self-test fails when a required job, gate need, gate result check, deploy direct need, or target-dir opt-in is missing.
- **SC-002**: `just ci-lint-workflow` passes locally after implementation and prints no generic parse failures.
- **SC-003**: `fmt-check` no longer has `needs: detector`; `build` still has detector output gating.
- **SC-004**: Exact-head CI proves the final topology passes through the aggregate `gate`.
- **SC-005**: The PR body names residual #332/#195/#205/#344/#340 scope instead of silently treating those future topologies as complete.
- **SC-006**: The workflow hygiene self-test fails when CI regresses to source-building `cargo-deny`, `cargo-nextest`, or `cargo-zigbuild`, disables install-action fallback protection, or drops checksum verification for the `cargo-zigbuild` archive.
## Assumptions

- PR #346 / #342 is the stacked base for this work, so source-fence is active topology.
- `fmt-check` still needs the managed owner because `just fmt-check` runs managed Rust formatting.
- Avoiding managed target-dir resolution is worthwhile only when enforced by lint so future jobs do not drift.
