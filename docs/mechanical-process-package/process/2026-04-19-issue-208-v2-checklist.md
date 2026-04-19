# Issue #208 V2 Checklist

## Purpose

This document turns the current process lessons into a concrete follow-up checklist for `#208`.

The goal is not to add more prose.
The goal is to close the gap between:

- a thin artifact validator
- and a general-purpose mechanical delivery lane

for feature work that needs high confidence.

## Core Direction

The process should not hardcode `bolt-v2` policy into the engine.

Split the system into:

1. **Generic engine**
   - stage machine
   - artifact parser
   - validator pass order
   - canonical finding rules
   - fail-closed behavior

2. **Repo profile**
   - repo-specific policy
   - package root
   - CI surfaces
   - review providers
   - merge policy
   - artifact naming

3. **Per-deliverable package**
   - issue contract
   - seam contract
   - proof plan
   - findings
   - evidence
   - exact targets
   - merge claims

The engine should be reusable.
The profile should be configurable.
The deliverable state should be data.

## Why V2 Is Needed

The current package already proves useful things:

- seam lock can block semantic ambiguity before implementation
- proof planning can surface blocker classes before review
- stale review artifacts can be filtered mechanically
- a thin validator can enforce artifact-shape rules

But the `#205` run exposed missing mechanics:

1. the package can drift away from the exact implementation head
2. the package can drift away from the exact GitHub review target
3. CI proof can be sampled noisily from the PR surface instead of the exact run that matters
4. the process can claim a stronger guarantee than the implementation actually enforces
5. fail-closed behavior can be semantically correct but operationally unreachable because of CI platform semantics
6. non-correctness review notes need a clean canonical home

Those are the V2 gaps.

## Artifact Additions

### 1. `profile.toml`

### Role

This is the repo/profile layer.
It moves repo policy out of the generic validator.

### Required fields

- `profile_name`
- `repo`
- `package_root`
- `default_stage_order`
- `review_providers`
- `relevant_workflows`
- `ignored_pr_surface_checks`
- `default_merge_policy`
- `artifact_naming_rules`

### Example fields

- `package_root = "docs/mechanical-process-package"`
- `review_providers = ["human", "gemini", "greptile", "glm", "claude"]`
- `relevant_workflows = ["CI"]`
- `ignored_pr_surface_checks = ["nt-pointer-trust-root"]`

### Why it exists

The engine should not hardcode:

- `bolt-v2`
- `main`
- `CI`
- `bolt-v2-binary`
- local docs paths

Those belong in the profile.

### 2. `execution_target.toml`

### Role

Freeze the exact implementation state that the package claims to describe.

### Required fields

- `repo`
- `branch`
- `base_ref`
- `head_sha`
- `diff_identity`
- `changed_paths`
- `status`

### Why it exists

`review_target.toml` is not enough.
The package also needs a mechanical binding to the actual implementation head, not just one review round.

### 3. `ci_surface.toml`

### Role

Define which CI evidence actually counts for this deliverable.

### Required fields

- `workflow`
- `head_sha`
- `run_selection_rule`
- `required_jobs_by_stage`
- `ignored_jobs`
- `partial_ci_allowed_stages`
- `terminal_ci_required_stages`

### Why it exists

The aggregate PR check surface is too noisy.
The process needs a declared answer to:

- which run counts
- which jobs count
- which unrelated failures are noise
- whether a partial exact-head snapshot is acceptable at `review` stage

### 4. `claim_enforcement.toml`

### Role

Prevent overclaiming.

### Required fields

- `claim_id`
- `enforcement_kind`
- `enforced_at`
- `test_ref`
- `ci_ref`
- `evidence_required`
- `status`

### Allowed `enforcement_kind`

- `code`
- `workflow`
- `test`
- `ci`
- `assumption`

### Why it exists

The process must not assert a stronger guarantee than the implementation enforces.
Every strong claim needs a mechanical enforcement locus.

### 5. `assumption_register.toml`

### Role

Hold valid trust-boundary and environment assumptions in structured state.

### Required fields

- `assumption_id`
- `impact_class`
- `subject`
- `description`
- `trust_root`
- `monitor`
- `expiry_trigger`
- `status`

### Why it exists

Not every valid review note is a correctness bug.
Some are boundary accepts or trust-model assumptions.
Those should not be left in prose.

### 6. `review_rounds/<round_id>.toml`

### Role

Record each review ingestion round explicitly.

### Required fields

- `round_id`
- `source`
- `review_target_ref`
- `raw_comment_refs`
- `ingested_findings`
- `stale_findings`
- `wrong_target_findings`
- `absorbed_by_head`
- `status`

### Why it exists

GitHub thread UI state is not authoritative.
The process needs a round-based ledger of what was reviewed, what was stale, and what was absorbed on a later head.

### 7. `stage_promotion.toml`

### Role

Declare the stage transition and name the one authoritative gate artifact.
This applies to every stage, not only review-stage packages.

### Required fields

- `from_stage`
- `to_stage`
- `promotion_gate_artifact`

### 8. `promotion_gate.toml`

### Role

Hold the one mechanical exam that can admit the stage transition.
Every selected stage must bind through one of these artifacts.

### Required fields

- `gate_id`
- `from_stage`
- `to_stage`
- `comparator_kind`
- `left_ref`
- one of:
  - `right_ref`
  - `right_literal`
- `verdict`
- `status`

### Why it exists

Stages exist today, but promotion still depends too much on human interpretation and artifact checklists.
V2 should make promotion exclusive:

- exactly one promotion row per active stage
- `stage_promotion.toml` names exactly one `promotion_gate.toml`
- `promotion_gate.toml` contains exactly one gate
- the gate runs one declared comparator against bound artifacts or literals
- only `verdict = pass` may advance the stage

## Schema Additions

The current canonicalization rules already added `impact_class`.
V2 should extend the schema with:

- `discovered_at_stage`
- `expected_gate`
- `process_signal`
- `resolved_at_stage`
- `enforcement_ref`

### Expected values

#### `discovered_at_stage`

- `intake`
- `seam_locked`
- `proof_locked`
- `implementation`
- `review`
- `merge_candidate`

#### `expected_gate`

- `intake_lock`
- `seam_lock`
- `proof_plan_lock`
- `review_target_lock`
- `finding_resolution_gate`
- `merge_gate`

#### `process_signal`

- `new_blocker_class`
- `expected_review_note`
- `schema_gap`
- `stale_artifact`

#### `resolved_at_stage`

- `contract`
- `implementation`
- `review`
- `merge_candidate`

### Why this matters

The process must distinguish:

- resolved in design
- resolved in delivery

Those are not the same state.

## Validator Pass Additions

V2 should add these passes after the current validator surface.

### Pass 10: Execution Target Validation

Checks:

- `execution_target.toml` exists for implementation-stage and later deliverables
- package `head_sha` matches actual implementation head
- `changed_paths` match the declared diff scope

Failure examples:

- package bound to stale branch head
- implementation changed files outside declared scope

### Pass 11: CI Surface Validation

Checks:

- `ci_surface.toml` exists when CI evidence is required
- exact run selection matches profile rules
- required jobs exist for the current stage
- ignored PR-surface jobs do not contaminate deliverable truth

Failure examples:

- package cites aggregate PR checks instead of exact run
- required CI job missing or unbound

### Pass 12: Claim Enforcement Validation

Checks:

- every strong true claim has an enforcement row
- enforcement row references real loci
- no claim is stronger than its enforcement surface

Failure examples:

- claim has evidence but no actual enforcement locus
- claim says "attempt-bound" when implementation only binds run-level state

### Pass 13: Assumption Register Validation

Checks:

- every `boundary_accept` or trust-boundary note points to an assumption row
- each assumption names a trust root and a monitor

Failure examples:

- assumption accepted with no monitor
- trust-boundary note left as prose only

### Pass 14: Review Round Validation

Checks:

- every external review corpus is ingested into a round file
- each round binds to an exact review target
- absorbed findings reference the head that absorbed them

Failure examples:

- live review comments with no ingestion round
- unresolved stale comments treated as active findings

### Pass 15: Stage Promotion Validation

Checks:

- exactly one promotion row exists for the active stage
- the promotion row names exactly one promotion gate artifact
- the promotion gate artifact exists
- the promotion gate artifact contains exactly one gate
- that gate is bound to the same from/to stage
- that gate comparator resolves and passes
- no later-stage artifact exists without required earlier-stage prerequisites

Failure examples:

- implementation admitted through artifact accumulation instead of one declared gate
- merge candidate claimed before exact-head CI terminal evidence exists

### Pass 16: Orchestration Reachability Validation

Checks:

- fail-closed fallback paths are reachable under all relevant upstream job states
- dependency semantics do not make the fallback dead code

Failure examples:

- semantic fallback exists but CI platform skip/fail behavior makes it unreachable

## Command Additions

### `just pre-push-issue-gate`

This should run:

- formatter
- issue-local tests
- issue-local lint/contract checks
- validator on the current package/stage

### Why it exists

This removes avoidable review churn from hygiene misses like repeated fmt-check failures.

## Success Criteria For Production-Ready

Do not call the process production-ready until all are true:

1. the validator exists and enforces V2 artifact rules
2. one issue completes with:
   - exact implementation target bound
   - exact CI surface bound
   - exact review rounds ingested
   - no late new correctness blocker class
3. one second issue of a different shape also completes cleanly
4. schema evolution, if any, is versioned and replayed on the corpus

## Priority Order

Implement V2 in this order:

1. `execution_target.toml`
2. `ci_surface.toml`
3. `claim_enforcement.toml`
4. `assumption_register.toml`
5. `review_rounds/`
6. stage-split finding resolution
7. `just pre-push-issue-gate`
8. orchestration-reachability validation
9. `stage_promotion.toml`
10. `promotion_gate.toml`

## Short Verdict

The next step for `#208` is not a bigger platform.

It is to extend the thin validator into a thin generic engine that can mechanically bind:

- package
- implementation head
- CI surface
- review corpus
- merge truth

without hardcoding repo-specific policy into the engine itself.
