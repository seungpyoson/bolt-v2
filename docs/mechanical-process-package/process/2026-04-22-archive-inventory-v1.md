# Archive Inventory v1

## Purpose

This file is the single clean inventory for the saved `#208` process work.

It tells you:

- which branches/tags preserve the work
- which branch is the primary doc archive
- what docs exist by category
- what is unique to the subject branch

## Saved Refs

Primary archive branch:

- `issue-208-validation-protocol`

Subject-under-test branch:

- `issue-208-scientific-validation-post-protocol`

Prototype baseline branch:

- `issue-208-process-validator`

Archive tags:

- `issue-208-process-validator-c4d182d`
- `issue-208-scientific-validation-subject-50c0fca`
- `issue-208-validation-protocol-b6f24d1`
- `issue-208-scientific-validation-post-protocol-6b36d16`

Use the branches as moving archive entry points.
Use the tags as immutable checkpoints.

## Archive Summary

`issue-208-validation-protocol` is the main archive branch.

It contains:

- the full process history
- the protocol artifacts
- the benchmark artifacts
- the adjudication artifacts
- the handoff prompts
- the full `candidate-205` package
- the earlier experiments

Doc count on `issue-208-validation-protocol`:

- total docs under `docs/mechanical-process-package/`: `100`
- `process/`: `45`
- `validation/`: `10`
- `candidate-205-smoke-tag-ci/`: `22`
- `experiments/`: `22`
- root README: `1`

`issue-208-scientific-validation-post-protocol` is not the main archive branch.

It primarily preserves:

- the replayed subject state
- the post-protocol subject registration artifact

Doc count on `issue-208-scientific-validation-post-protocol`:

- total docs under `docs/mechanical-process-package/`: `70`
- `process/`: `24`
- `validation/`: `1`
- `candidate-205-smoke-tag-ci/`: `22`
- `experiments/`: `22`
- root README: `1`

## Primary Archive Inventory

### Root

- `docs/mechanical-process-package/README.md`

### Candidate #205 Package

- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/README.md`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/assumption_register.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/ci_surface.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/claim_enforcement.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/claim_enforcement_coverage.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/decision_packet.md`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/evidence_bundle.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/execution_target.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/finding_ledger.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/implementation_plan.md`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/issue_contract.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/merge_claims.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/orchestration_reachability.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/orchestration_reachability_summary.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/promotion_gate.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/proof_plan.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/result.md`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/review_rounds/pr-210-r2.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/review_target.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/seam_contract.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/stage_promotion.toml`
- `docs/mechanical-process-package/candidate-205-smoke-tag-ci/workflow_reachability_contract.toml`

### Experiments

#### `exp-eth-anchor-semantics`

- `docs/mechanical-process-package/experiments/exp-eth-anchor-semantics/README.md`
- `docs/mechanical-process-package/experiments/exp-eth-anchor-semantics/evidence_bundle.toml`
- `docs/mechanical-process-package/experiments/exp-eth-anchor-semantics/finding_ledger.toml`
- `docs/mechanical-process-package/experiments/exp-eth-anchor-semantics/issue_contract.toml`
- `docs/mechanical-process-package/experiments/exp-eth-anchor-semantics/merge_claims.toml`
- `docs/mechanical-process-package/experiments/exp-eth-anchor-semantics/proof_plan.toml`
- `docs/mechanical-process-package/experiments/exp-eth-anchor-semantics/result.md`
- `docs/mechanical-process-package/experiments/exp-eth-anchor-semantics/seam_contract.toml`

#### `exp-finding-canonicalization`

- `docs/mechanical-process-package/experiments/exp-finding-canonicalization/README.md`
- `docs/mechanical-process-package/experiments/exp-finding-canonicalization/evidence_bundle.toml`
- `docs/mechanical-process-package/experiments/exp-finding-canonicalization/finding_ledger.toml`
- `docs/mechanical-process-package/experiments/exp-finding-canonicalization/merge_claims.toml`
- `docs/mechanical-process-package/experiments/exp-finding-canonicalization/result.md`
- `docs/mechanical-process-package/experiments/exp-finding-canonicalization/review_target.toml`

#### `exp-proof-plan-selector-path`

- `docs/mechanical-process-package/experiments/exp-proof-plan-selector-path/README.md`
- `docs/mechanical-process-package/experiments/exp-proof-plan-selector-path/evidence_bundle.toml`
- `docs/mechanical-process-package/experiments/exp-proof-plan-selector-path/finding_ledger.toml`
- `docs/mechanical-process-package/experiments/exp-proof-plan-selector-path/issue_contract.toml`
- `docs/mechanical-process-package/experiments/exp-proof-plan-selector-path/merge_claims.toml`
- `docs/mechanical-process-package/experiments/exp-proof-plan-selector-path/proof_plan.toml`
- `docs/mechanical-process-package/experiments/exp-proof-plan-selector-path/result.md`
- `docs/mechanical-process-package/experiments/exp-proof-plan-selector-path/seam_contract.toml`

### Process Docs

- `docs/mechanical-process-package/process/2026-04-19-blocker-class-to-gate-map-v1.md`
- `docs/mechanical-process-package/process/2026-04-19-finding-canonicalization-rules-v1.md`
- `docs/mechanical-process-package/process/2026-04-19-fresh-issue-candidates-v1.md`
- `docs/mechanical-process-package/process/2026-04-19-fresh-issue-experiment-protocol-v1.md`
- `docs/mechanical-process-package/process/2026-04-19-issue-208-v2-checklist.md`
- `docs/mechanical-process-package/process/2026-04-19-mechanical-delivery-process-v1.md`
- `docs/mechanical-process-package/process/2026-04-19-process-status-matrix-v1.md`
- `docs/mechanical-process-package/process/2026-04-19-validator-fixtures-v1.md`
- `docs/mechanical-process-package/process/2026-04-19-validator-spec-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-adjudication-results-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-adjudication-rules-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-benchmark-manifest-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-ci-surface-comparator-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-claim-enforcement-closure-boundary-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-claim-enforcement-replay-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-claim-enforcement-scalarization-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-claude-continuation-handoff-v2.md`
- `docs/mechanical-process-package/process/2026-04-20-claude-ultraplan-handoff-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-closure-boundary-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-comparator-closure-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-comparator-closure-experiment-v2.md`
- `docs/mechanical-process-package/process/2026-04-20-completeness-gate-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-experiment-ledger-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-external-adjudication-packet-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-full-reachability-replay-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-future-scientific-validation-entry-gates-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-producer-contract-schema-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-reachability-replay-boundary-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-reachability-replay-reduction-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-reachability-scalarization-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-review-exact-head-comparator-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-review-round-completeness-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-scalar-summary-schema-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-adjudication-packet-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-adjudication-results-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-adjudication-results-v2.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-adjudication-status-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-adjudication-status-v2.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-results-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-results-v2.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-review-prompt-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-review-prompt-v2.md`
- `docs/mechanical-process-package/process/2026-04-20-summary-replay-module-experiment-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-validation-protocol-v1.md`
- `docs/mechanical-process-package/process/2026-04-20-validation-protocol-v1.toml`

### Validation Docs

#### Benchmarks

- `docs/mechanical-process-package/validation/benchmarks/B4-unsupported-comparator-kinds.toml`
- `docs/mechanical-process-package/validation/benchmarks/B5-scalar-summary-schema-breaks.toml`
- `docs/mechanical-process-package/validation/benchmarks/B6-summary-replay-drift.toml`
- `docs/mechanical-process-package/validation/benchmarks/B7-producer-contract-schema-breaks.toml`
- `docs/mechanical-process-package/validation/benchmarks/README.md`
- `docs/mechanical-process-package/validation/benchmarks/descriptor-template-v1.toml`
- `docs/mechanical-process-package/validation/benchmarks/fixture-sets/review-package-protocol-owned.toml`
- `docs/mechanical-process-package/validation/benchmarks/runners/held-out-b4-b7-review-runner.toml`

#### Subject Templates

- `docs/mechanical-process-package/validation/subjects/README.md`
- `docs/mechanical-process-package/validation/subjects/subject-registration-template-v1.toml`

## Subject Branch Unique Doc

The subject branch adds one unique archive doc that is not on the protocol branch:

- `docs/mechanical-process-package/validation/subjects/2026-04-20-post-protocol-subject-registration-v1.toml`

## Practical Reading Order

If you only want the shortest archive path:

1. `docs/mechanical-process-package/README.md`
2. `docs/mechanical-process-package/process/2026-04-20-validation-protocol-v1.toml`
3. `docs/mechanical-process-package/process/2026-04-20-future-scientific-validation-entry-gates-v1.toml`
4. `docs/mechanical-process-package/process/2026-04-20-scientific-validation-adjudication-results-v2.toml`
5. `docs/mechanical-process-package/process/2026-04-20-claude-continuation-handoff-v2.md`
