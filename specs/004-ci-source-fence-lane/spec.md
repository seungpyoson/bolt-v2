# Feature Specification: CI Source-Fence Lane

**Feature Branch**: `codex/ci-342-source-fence`
**Created**: 2026-05-15
**Status**: Draft
**Input**: User description: "Address #342 from epic #333 without narrowing the issue body."

## User Scenarios & Testing

### User Story 1 - Source-Fence Drift Fails Early (Priority: P1)

As the maintainer, I can see deterministic Bolt-v3 source-fence and verifier failures in a dedicated `source-fence` CI lane before the expensive full Rust test lane dominates the run.

**Why this priority**: Run `25859831755` proved a stale source-fence assertion was found late inside the monolithic `test` lane. #342 exists to surface that class first.

**Independent Test**: Introduce a deliberate stale source-fence assertion locally and confirm the `source-fence` recipe fails before `just test` or `cargo nextest` is required.

**Acceptance Scenarios**:

1. **Given** a source-fence test searches for stale production source shape, **When** the `source-fence` lane runs, **Then** the lane fails on that filter without installing `cargo-nextest` or running full `just test`.
2. **Given** the workflow reaches the full `test` job, **When** `source-fence` has not succeeded, **Then** `test` is blocked or skipped and the aggregate `gate` fails closed.
3. **Given** the source-fence filters still also exist in full `nextest`, **When** #332 has not landed yet, **Then** the duplicate execution is explicitly documented as intentional and temporary under one aggregate gate.

### User Story 2 - Verifier Script Set Is Complete (Priority: P1)

As the maintainer, I can run the exact verifier script list named by #342 as one deterministic command before full Rust tests.

**Why this priority**: The #342 body names six verifier scripts. Two are missing on the #343 baseline branch; silently dropping them would narrow the issue.

**Independent Test**: `just source-fence` runs all six verifier scripts successfully on the branch head.

**Acceptance Scenarios**:

1. **Given** the source-fence recipe starts, **When** it runs verifier scripts, **Then** it invokes `verify_bolt_v3_runtime_literals.py`, `verify_bolt_v3_provider_leaks.py`, `verify_bolt_v3_core_boundary.py`, `verify_bolt_v3_naming.py`, `verify_bolt_v3_status_map_current.py`, and `verify_bolt_v3_pure_rust_runtime.py`.
2. **Given** a named verifier script is missing or stale, **When** the recipe runs, **Then** it fails before any full test suite command.
3. **Given** a verifier needs Python, **When** it runs in CI, **Then** it must use only deterministic repo or standard-library dependencies.

### User Story 3 - Gate And Linter Know The New Lane (Priority: P1)

As the maintainer, I can rely on `gate` and `just ci-lint-workflow` to fail closed if the source-fence lane is missing, skipped unexpectedly, cancelled, failed, timed out, or disconnected from the PR head.

**Why this priority**: #203 is still open, so #342 must carry its own narrow invariant update instead of shipping an unlinted topology change.

**Independent Test**: Remove the `source-fence` job, remove its gate result check, or remove the `test` dependency on it and confirm `just ci-lint-workflow` reports the specific missing invariant.

**Acceptance Scenarios**:

1. **Given** `.github/workflows/ci.yml` is edited, **When** the `source-fence` job is missing, **Then** `just ci-lint-workflow` fails with an actionable message.
2. **Given** `gate` does not require `source-fence`, **When** the linter runs, **Then** it fails before CI accepts the topology.
3. **Given** the `source-fence` job is cancelled, skipped unexpectedly, timed out, failed, missing, or stale for the workflow execution, **When** `gate` evaluates `needs.source-fence.result`, **Then** only `success` passes.

## Edge Cases

- #332 has not landed yet, so full `just test` still duplicates the source-fence filters. The branch must document that temporary duplicate ownership instead of silently treating #332 as done.
- `source-fence` must compile the targeted integration tests through normal `cargo test`; that compile cost is accepted, but full `cargo nextest` install and execution are not part of this lane.
- GitHub Actions job failures do not automatically cancel independent jobs. The workflow must make full `test` depend on `source-fence` so stale source-fence drift does not run after expensive test setup.
- Python verifier dependencies must not depend on unpinned runner image packages.
- Docs-only or path-filtered PRs are owned by #335/#344. This slice does not change path filters.

## Requirements

### Functional Requirements

- **FR-001**: The workflow MUST add a top-level CI job named `source-fence`.
- **FR-002**: `source-fence` MUST run after `detector` and before the full `test` job can start.
- **FR-003**: `source-fence` MUST run exactly the verifier script set named by #342: runtime literals, provider leaks, core boundary, naming, status map current, and pure Rust runtime.
- **FR-004**: `source-fence` MUST run the canonical structural test filters `bolt_v3_controlled_connect live_node_module_only_runs_nt_after_live_canary_gate` and `bolt_v3_production_entrypoint`.
- **FR-005**: `source-fence` MUST avoid full `cargo-nextest` installation and full integration test execution.
- **FR-006**: The aggregate `gate` job MUST include `source-fence` in `needs` and MUST accept only `needs.source-fence.result == "success"`.
- **FR-007**: The workflow linter MUST fail with actionable output when `source-fence` is missing from jobs, missing from `gate.needs`, missing from the gate result check, missing from `test.needs`, missing cache ownership, or missing managed setup.
- **FR-008**: The branch MUST explicitly document the temporary duplicate execution of source-fence filters in full `test` until #332 either excludes them from nextest shards or records an intentional duplicate-run choice.
- **FR-009**: `verify_bolt_v3_pure_rust_runtime.py` MUST enforce the no-PyO3/no-maturin/no-Python-runtime boundary for production Rust code and build metadata while allowing Python CI verifier tooling.
- **FR-010**: `verify_bolt_v3_status_map_current.py` MUST enforce status-map evidence hygiene, including existing referenced verifier paths and a current row for the pure Rust runtime verifier.
- **FR-011**: Exact-head CI evidence MUST show `source-fence`, `fmt-check`, `deny`, `clippy`, `test`, and `gate` passing on the final PR head.

### Key Entities

- **SourceFenceJob**: GitHub Actions job that runs the structural verifier recipe and owns early failure for source-scan drift.
- **SourceFenceRecipe**: `just source-fence`, the local and CI command that runs verifiers and targeted cargo test filters.
- **VerifierScriptSet**: Six Python verifier scripts named by #342.
- **GateInvariant**: Workflow and linter checks proving `source-fence` is required and fail-closed.
- **TemporaryDuplicateOwnershipNote**: Documentation that #342 owns the canonical filters now, while #332 will later exclude them from full nextest shards or explicitly keep duplicate execution under the aggregate gate.

## Success Criteria

### Measurable Outcomes

- **SC-001**: `just ci-lint-workflow` fails before workflow implementation when the new #342 invariant is absent, then passes after implementation.
- **SC-002**: `just source-fence` passes locally and invokes all six verifier scripts plus both canonical cargo test filters.
- **SC-003**: A deliberate stale source-fence assertion fails through `just source-fence` without running full `just test`.
- **SC-004**: Exact-head CI shows a successful `source-fence` job alongside existing successful `fmt-check`, `deny`, `clippy`, `test`, and `gate` jobs.
- **SC-005**: No workflow path-filter, #332 sharding, #195 cache-retention, #205 deploy-dedup, #335 docs-only, #344 operational, or #340 config-path work is implemented in this slice.

## Assumptions

- #342 intentionally lands before #332, so a temporary duplicate run of the source-fence filters inside full `test` is acceptable only if it is explicit and gate-covered.
- The dedicated lane may add warm success wall time before `test`; current #343 baseline shows `clippy` remains the PR critical path before #332.
- The new verifier scripts are required because the #342 issue body names them.
