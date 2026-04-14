# Issue 159 NT Pointer Probe Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the frozen NT pointer probe design in `docs/superpowers/specs/2026-04-14-nt-pointer-probe-design.md` into a fail-closed solo-operator probe system that blocks unsafe NT pin movement and only opens tagged-release draft PRs when evidence is complete.

**Architecture:** Keep the probe logic repo-owned and Rust-first, separate control-plane data from GitHub orchestration, and reuse the existing managed `just` plus GitHub Actions setup flow instead of inventing a parallel build path. Land control-plane safety rails before merge-candidate automation, and keep the `develop` lane advisory-only all the way through rollout.

**Tech Stack:** Rust 2024, GitHub Actions, existing `just` recipes and managed Rust verification owner, TOML/JSON control artifacts, GitHub issue/PR APIs, current Bolt test suite as canaries, durable artifact storage outside PRs.

---

## 1. Implementation phases

### Phase 1: Repo / Configuration Changes

Purpose: establish one repo-owned source of truth for NT probe policy, seam ownership, safe-list entries, replay fixtures, and lane settings before any workflow starts making decisions.

Likely files:
- Create `config/nt_pointer_probe/control.toml`
- Create `config/nt_pointer_probe/registry.toml`
- Create `config/nt_pointer_probe/safe_list.toml`
- Create `config/nt_pointer_probe/replay_set.toml`
- Create `config/nt_pointer_probe/expected_branch_protection.{toml,json}`
- Modify `Cargo.toml`
- Modify `Cargo.lock`
- Modify `justfile`
- Modify `src/lib.rs`
- Create `src/nt_pointer_probe/mod.rs`
- Create `tests/fixtures/nt_pointer_probe/`

Dependencies on earlier slices:
- None. This is the base slice.

Main risks:
- Spreading policy values between TOML and workflow YAML, which would violate the repo’s no-hardcodes rule and create drift.
- Picking a registry and safe-list shape that does not map cleanly onto the existing Bolt tests and `nautilus-*` usage inventory.
- Treating the illustrative seam list in the spec as complete before the one-time completeness audit is real.

Verification approach:
- Add parser and schema tests for every new control file format.
- Add one repo-local validation command in `justfile` that loads all control files and fails on missing required fields, invalid expiry windows, or unknown coverage classes.
- Add a fixture-backed test that proves the expected-branch-protection artifact can be parsed and normalized deterministically.

### Phase 2: GitHub Workflow / CI Changes

Purpose: create dedicated workflow lanes and self-tests around the probe without weakening the current `ci.yml` contract or adding a second build path.

Likely files:
- Create `.github/workflows/nt-pointer-probe-develop.yml`
- Create `.github/workflows/nt-pointer-probe-tagged.yml`
- Create `.github/workflows/nt-pointer-probe-self-test.yml`
- Create `.github/workflows/nt-pointer-control-plane.yml`
- Modify `.github/workflows/ci.yml`
- Modify `.github/workflows/advisory.yml`
- Modify `.github/dependabot.yml`
- Modify `.github/workflows/dependabot-auto-merge.yml`
- Modify `.github/actions/setup-environment/action.yml`
- Modify `justfile`

Dependencies on earlier slices:
- Depends on Phase 1 for config paths, status-check names, and validator command names.

Main risks:
- Hardcoding lane values, status names, or label strings directly in YAML.
- Duplicating setup logic instead of reusing `.github/actions/setup-environment/action.yml`.
- Enabling a workflow that can create PRs before the fail-closed self-test and control-plane checks exist.

Verification approach:
- Extend `just ci-lint-workflow` so new workflows must use the managed setup action and pinned values.
- Add a self-test workflow with known-good and known-bad fixtures for artifact validation and fail-closed behavior.
- Run every new workflow first via `workflow_dispatch` in report-only mode before making any status check required.

### Phase 3: Probe Logic And Artifact Generation

Purpose: implement the deterministic probe engine that resolves an upstream NT ref once, updates all pinned `nautilus-*` crates atomically, inventories Bolt-owned NT use, classifies upstream paths, runs required canaries, and emits one atomic evidence artifact.

Likely files:
- Create `src/bin/nt_pointer_probe.rs` or extend `src/main.rs` with a dedicated probe command surface
- Create `src/nt_pointer_probe/control.rs`
- Create `src/nt_pointer_probe/upstream.rs`
- Create `src/nt_pointer_probe/inventory.rs`
- Create `src/nt_pointer_probe/classify.rs`
- Create `src/nt_pointer_probe/inference.rs`
- Create `src/nt_pointer_probe/evidence.rs`
- Create `src/nt_pointer_probe/github.rs`
- Modify `src/lib.rs`
- Modify `Cargo.toml`
- Modify `Cargo.lock`
- Create `tests/nt_pointer_probe_control.rs`
- Create `tests/nt_pointer_probe_evidence.rs`
- Create `tests/nt_pointer_probe_replay.rs`
- Modify existing canary-bearing tests such as `tests/polymarket_bootstrap.rs`, `tests/render_live_config.rs`, `tests/reference_actor.rs`, `tests/reference_pipeline.rs`, `tests/platform_runtime.rs`, `tests/normalized_sink.rs`, `tests/lake_batch.rs`, `tests/raw_capture_transport.rs`, and `tests/live_node_run.rs` where the registry audit finds coverage gaps

Dependencies on earlier slices:
- Depends on Phase 1 for registry, safe-list, replay-set, and branch-protection config formats.
- Depends on Phase 2 for the workflow shell that will invoke the probe.

Main risks:
- Resolving the upstream ref more than once and accidentally mixing SHAs in one run.
- Letting advisory inference weaken registry decisions instead of only tightening them.
- Recording a passing artifact that does not prove which canaries actually ran or which coverage classes were satisfied.
- Underestimating the amount of canary gap work needed once the completeness audit maps every `nautilus_*` use site.

Verification approach:
- Add unit tests for ref resolution, path classification, safe-list invalidation, and artifact sealing.
- Add golden-file tests for the evidence artifact schema and digest stability.
- Add replay tests proving historical dangerous patterns still escalate.
- Add a local dry-run command that runs against the current pinned SHA and produces an artifact without opening issues or PRs.

### Phase 4: Control-Plane Protections

Purpose: close side-door mutation paths so manual edits, Dependabot, and workflow changes cannot bypass the NT probe policy.

Likely files:
- Modify `.github/dependabot.yml`
- Modify `.github/workflows/dependabot-auto-merge.yml`
- Modify `.github/workflows/nt-pointer-control-plane.yml`
- Modify `.github/workflows/ci.yml`
- Modify `justfile`
- Modify `config/nt_pointer_probe/control.toml`
- Modify `config/nt_pointer_probe/expected_branch_protection.{toml,json}`
- Extend `src/nt_pointer_probe/control.rs`
- Extend `src/nt_pointer_probe/github.rs`
- Create `tests/nt_pointer_probe_control_plane.rs`

Dependencies on earlier slices:
- Depends on Phase 1 for controlled artifact locations and expected-state definitions.
- Depends on Phase 2 for dedicated PR and scheduled workflows.
- Depends on Phase 3 for artifact identity checks if PRs touching NT pins must prove probe origin.

Main risks:
- False-positive blocking on legitimate control-plane edits, especially in a solo repo.
- Relying on labels, comments, or branch names instead of comparing exact changed lines and artifact metadata.
- Treating GitHub branch-protection drift as informational instead of gating, which would make the rest of the controls optional in practice.

Verification approach:
- Add fixture tests for allowed and blocked PR shapes: manual NT pin edit, registry-only edit, safe-list edit, workflow edit, and valid probe-generated bump.
- Add a scheduled drift check that compares live branch protection to the expected-state artifact and leaves a durable failure record.
- Validate that Dependabot can still update ordinary Cargo and Actions dependencies while `nautilus-*` updates are excluded from the auto-merge path.

### Phase 5: Develop-Lane Advisory Output

Purpose: make the `develop` lane useful early-warning infrastructure without creating any merge path.

Likely files:
- Modify `.github/workflows/nt-pointer-probe-develop.yml`
- Extend `src/nt_pointer_probe/github.rs`
- Extend `src/nt_pointer_probe/evidence.rs`
- Create `src/nt_pointer_probe/advisory.rs`
- Modify `config/nt_pointer_probe/control.toml`
- Create `.github/nt-pointer-probe/advisory_issue.md`
- Create `tests/nt_pointer_probe_advisory.rs`

Dependencies on earlier slices:
- Depends on Phase 2 for workflow invocation.
- Depends on Phase 3 for artifact generation and summary rendering.
- Depends on Phase 4 so the advisory lane cannot be mistaken for an authorized merge path.

Main risks:
- Creating multiple advisory issues because the matching logic is not strict enough.
- Letting the issue accumulate comments forever instead of maintaining one current body.
- Failing to mark develop results stale or superseded when a tagged-release result becomes authoritative.

Verification approach:
- Add tests for issue matching, reopen behavior, duplicate detection, and body replacement.
- Run a manual `workflow_dispatch` against the current pinned SHA and verify that only the advisory issue updates.
- Confirm in GitHub that the develop lane never creates a PR, even on a fully passing run.

### Phase 6: Tagged-Release Merge-Candidate Flow

Purpose: add the calmer release lane that tracks the newest eligible NT tag, enforces soak, opens at most one draft PR, and leaves the external-review gate pending until the human operator records the out-of-band review artifact.

Likely files:
- Modify `.github/workflows/nt-pointer-probe-tagged.yml`
- Extend `src/nt_pointer_probe/upstream.rs`
- Extend `src/nt_pointer_probe/github.rs`
- Create `src/nt_pointer_probe/soak.rs`
- Create `src/nt_pointer_probe/state.rs`
- Modify `config/nt_pointer_probe/control.toml`
- Create `.github/nt-pointer-probe/draft_pr.md`
- Create `tests/nt_pointer_probe_tagged.rs`

Dependencies on earlier slices:
- Depends on Phase 3 for full evidence generation.
- Depends on Phase 4 for control-plane gating.
- Depends on Phase 5 for the already-proven GitHub reporting path.

Main risks:
- Incorrect soak accounting when a tag name resolves to a different SHA on a later observation.
- Accidentally opening a non-draft PR or multiple concurrent merge-candidate PRs.
- Closing or superseding the wrong PR when a newer probe run arrives.
- Allowing the external-review status to go green without a durable artifact keyed to the exact probe artifact and NT SHA.

Verification approach:
- Add state-machine tests for soak progression, soak reset, stale PR closure, and single-open-PR enforcement.
- Run one manual historical-tag rehearsal in dry-run mode to prove the PR body renders correctly without opening it.
- Run one live tagged-release dry run that creates a draft PR and confirm the external-review status remains pending or failing by default.

### Phase 7: Rollout And Verification

Purpose: activate the probe in small reversible steps, prove the safety rails with real runs, and only then make the relevant status checks required on `main`.

Likely files:
- Modify `.github/workflows/nt-pointer-probe-develop.yml`
- Modify `.github/workflows/nt-pointer-probe-tagged.yml`
- Modify `.github/workflows/nt-pointer-control-plane.yml`
- Modify `config/nt_pointer_probe/control.toml`
- Modify `config/nt_pointer_probe/expected_branch_protection.{toml,json}`
- Create `docs/nt-pointer-probe-rollout.md`
- Update `docs/superpowers/plans/2026-04-14-issue-159-nt-pointer-probe-implementation-plan.md` if activation steps need explicit follow-up notes

Dependencies on earlier slices:
- Depends on all earlier slices.

Main risks:
- Turning on required checks before the replay set, registry completeness audit, and advisory rehearsals are trustworthy.
- Silent workflow decay after rollout if alerting only lives in one channel or only on the happy path.
- Solo-operator fatigue leading to bypass pressure if the rollout jumps straight from zero automation to hard-required merge gates.

Verification approach:
- Start with report-only scheduled runs and manual dispatch.
- Promote to advisory-only `develop` automation next.
- Promote to tagged draft-PR creation only after one full end-to-end dry run proves soak, artifact durability, and PR supersession behavior.
- Make branch-protection requirements final only after the required statuses are demonstrably stable and the drift check is clean.

## 2. Ordering and dependencies

1. Land Phase 1 first. The repo rules require runtime and policy values to come from config, and every later slice needs stable control-file paths and schemas.
2. Pull the minimum safety pieces of Phases 2 and 4 forward immediately after Phase 1. In practice that means Dependabot exclusion for `nautilus-*`, a control-plane validator, the workflow self-test shell, and a branch-protection expected-state artifact.
3. Build Phase 3 next, but keep it dry-run and artifact-only until the completeness audit proves the registry, safe-list, replay set, and canary inventory are real.
4. Turn on Phase 5 before Phase 6. The `develop` lane is the lower-risk place to prove ref resolution, classification, artifact durability, and GitHub reporting without creating a merge surface.
5. Turn on Phase 6 only after the advisory lane is stable and the control-plane checks are already blocking side-door mutations.
6. Finish with Phase 7 staged rollout. Required status checks and draft-PR automation should be the last step, not the first.

Dependency notes:
- Phase 3 is blocked on Phase 1 schema decisions.
- Phase 4 is partially independent, but the strongest guardrails need Phase 3 artifact identity and Phase 2 workflow plumbing.
- Phase 5 depends on a passing Phase 3 artifact path and on Phase 4 ensuring advisory-only behavior cannot drift into merge authority.
- Phase 6 depends on Phase 5 plus a durable soak-state implementation.

## 3. Early-value subset

The strongest safety value fastest is not the full probe. It is the smallest set of changes that closes unsafe mutation paths before automation gains any authority.

Implement first:
- Phase 1 control files for lane settings, registry, safe-list, replay set, and expected branch protection.
- Phase 4 guards that exclude `nautilus-*` from Dependabot auto-update or auto-merge paths, validate control-plane files on every PR, and detect branch-protection drift.
- The report-only parts of Phase 2, especially the workflow self-test and managed setup wiring.

Why this subset first:
- It immediately blocks the easiest bypasses: manual NT pin bumps, bot NT pin bumps, and weakening the registry or workflow controls without detection.
- It is small and reversible. Most of the changes are config, validation, and workflow shell work rather than deeper runtime logic.
- It lowers implementation risk for the full probe because later slices can assume the control plane is already explicit and mechanically checked.

The next best early-value increment is a dry-run version of Phase 3 plus Phase 5 advisory output. That proves the end-to-end evidence path without yet creating draft PRs.

## 4. Open implementation choices

- Durable artifact store: the spec requires storage outside the PR with real retention. The practical repo-grounded choice is whether to reuse the existing AWS OIDC plus S3 pattern already present in `.github/workflows/ci.yml`, or introduce a different durable store.
- Probe CLI surface: decide between a dedicated Rust binary such as `src/bin/nt_pointer_probe.rs` and a new `bolt-v2` subcommand in `src/main.rs`. The spec does not force one, but the choice affects test surface and maintenance cost.
- Branch-protection drift check mechanism: the spec requires comparison to an expected-state artifact, but the exact normalized fields and whether the expected artifact is TOML or JSON still need to be fixed.
- Machine-checkable safe-list condition format: the spec requires conditions the probe can re-evaluate every run, but the exact expression format still needs to be pinned down.
- Soak-state persistence: the tagged lane needs durable day-over-day observations of tag name to SHA stability. The exact persistence location still needs to be chosen.
- External-review artifact location and status transition path: the spec requires a durable external-review artifact and a required status check, but the exact artifact path and the mechanism that flips the status to passing still need to be selected.
- Upstream NT source acquisition path: the probe needs a reproducible way to classify changed upstream paths. The exact acquisition method should be fixed so every run sees the same tree shape and diff basis.

## 5. Verification strategy

- Schema verification: every control artifact should have parser tests plus one repo-local validation command that CI and workflows both call.
- Registry completeness verification: add a deterministic inventory check that enumerates all direct `nautilus-*` dependencies and Bolt-side `nautilus_*` use sites, then fails if any lack seam ownership or if any registered upstream prefix no longer matches real NT paths.
- Evidence verification: treat the artifact as a contract. Add golden tests for schema stability, digest calculation, supersession metadata, and coverage-class recording.
- Replay verification: keep a historical replay set of known-dangerous upstream changes and run it on every inference or classification rule change and on a bounded schedule.
- Workflow verification: add a dedicated fail-closed self-test workflow with known-good and known-bad fixtures, and make workflow changes prove they still reject malformed evidence and ambiguous classifications.
- GitHub integration verification: rehearse advisory issue updates and tagged draft-PR creation in dry-run or controlled manual runs before any status becomes required.
- Rollout verification: require at least one full advisory rehearsal and one full tagged-release rehearsal with durable artifacts, clean supersession behavior, and an intentionally pending external-review gate before final activation.
- Baseline repo verification: keep using the current managed verification flow, including `just ci-lint-workflow` and `just test`, so the probe work does not create a second unverified automation lane.
