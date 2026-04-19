# Issue 182 Trust-Root Auth Design

## Status

Draft design contract for review before implementation.

## Goal

Fix `#182` by replacing the anonymous trust-root validator/policy fetch path with a private-repo-compatible authenticated path while preserving the existing trust-root execution model.

This design is intentionally narrow. It defines the invariants the implementation must satisfy and the proof the repo must carry after the change.

## Scope

This design includes:

- authenticated retrieval of the external trust-root validator bundle from private `claude-config`
- preservation of the `pull_request_target` trust-root model
- regression-proofing for the access-path and trust-boundary invariants

This design does **not** include:

- widening the trust-root workflow into a general CI/bootstrap lane
- checking out or executing PR-head repository code
- changing the broader NT pointer control-plane design
- redesigning unrelated workflows or secret-resolution systems

## Problem

The current trust-root workflow was originally designed around a narrow model:

- the workflow definition is owned by the protected base branch
- PR-head content is treated as data only
- an external validator bundle is pinned by exact commit SHA
- only policy-listed PR-head files are materialized and validated

Issue `#182` changes one part of that model: the external validator bundle can no longer be fetched anonymously from `raw.githubusercontent.com` because the source repo is private.

The design risk is not merely "fetch a private file." The real risk is solving that narrow access problem by expanding the trusted bootstrap surface before trust-root validation occurs.

## Design Contract

### 1. Base-Owned Execution

The trust-root workflow must remain a `pull_request_target` workflow.

The code and workflow logic that execute before trust-root validation must be owned by the protected base branch, not by the PR head. The PR head may contribute only file contents treated as data.

### 2. No PR-Head Execution

The trust-root workflow must not check out, run, import, or otherwise execute PR-head repository code.

Allowed PR-head interaction is limited to fetching exact file contents by `github.event.pull_request.head.sha` for paths authorized by the external trust-root policy.

### 3. Exact External Pinning

The external validator bundle must still be pinned by exact 40-character commit SHA.

The implementation must validate both:

- the configured validator reference is a commit SHA, not a branch, tag, or mutable ref
- the fetched object actually resolves to that exact SHA before validator or policy files are used

### 4. Narrow Trust Surface

The authenticated fetch path must remain a narrow trust-root primitive, not a general CI bootstrap.

The trust-root workflow may not depend on unrelated repo-local bootstrap layers, toolchain setup, lint orchestration, or secondary managed-owner installation unless those components are explicitly part of the trust-root authority boundary and are themselves protected by the same trust-root mechanism.

The intended model is:

- minimal authenticated fetch of the pinned external bundle
- materialization of policy-approved PR-head files as data
- execution of the pinned external validator against that staged data

### 5. Single Trust Boundary

The assets that define or execute the privileged trust-root decision must fit inside one clearly defined authority boundary.

If the workflow executes additional privileged local components before validation, those components must be treated as part of the trust-root boundary and must be protected, reviewed, and regression-tested accordingly. Silent expansion of that boundary is not allowed.

### 6. Secret Handling

Authentication material used for private fetches must not widen the credential exposure surface unnecessarily.

The design requirement is not a specific Git transport spelling. The requirement is that the chosen mechanism must avoid avoidable persistence or echo of secrets in workspace files, git config, or diagnostic output.

### 7. Semantic Regression Lock

The repo must carry self-tests that assert the trust-root invariants semantically, not just by freezing one incidental implementation spelling.

At minimum, the tests must prove:

- the workflow remains `pull_request_target`
- the workflow does not execute PR-head code
- the workflow uses an authenticated private fetch path for the external bundle
- the bundle fetch remains pinned by exact SHA and verifies the fetched SHA
- only policy-authorized PR-head files are materialized as data
- the implementation does not silently reintroduce anonymous bundle fetches
- the implementation does not silently widen the privileged bootstrap surface without test failure

### 8. Proof Model Must Match Runtime Model

Because `pull_request_target` evaluates the base-branch workflow definition on the PR surface, the repo cannot rely on PR-surface workflow success alone as proof that a changed trust-root workflow is correct.

The design must therefore include a proof path that meaningfully validates the post-merge runtime contract, rather than only validating string presence in the PR-head workflow file.

## Non-Goals

This design does not require:

- a full rewrite of all CI setup conventions
- elimination of all local actions from the repository
- changing branch protection or required status-check names beyond what is needed to preserve the trust-root contract
- solving broader secret-management concerns outside the trust-root fetch path

## Acceptance Boundary

Issue `#182` is complete when all of the following are true:

1. The trust-root validator and policy bundle are fetched from private `claude-config` through an authenticated path.
2. The workflow still uses `pull_request_target`.
3. The workflow still does not checkout or execute PR-head repository code.
4. The workflow still treats PR-head files as data only and only for policy-approved paths.
5. The external validator bundle remains pinned and verified by exact commit SHA.
6. The privileged trust-root execution boundary is no broader than the design explicitly allows.
7. The repo contains regression tests that would fail if the authenticated fetch path or trust-boundary invariants silently regress.

## Rejection Conditions

Any proposed implementation for `#182` must be rejected if it does any of the following:

- converts the workflow to `pull_request`
- checks out or executes PR-head code
- replaces exact SHA pinning with a mutable ref
- solves private access by importing a broad unrelated CI/bootstrap dependency into the trust-root path
- locks tests to incidental implementation spellings while leaving the real invariants unproven
- widens secret exposure as an unexamined side effect of the fetch-path change
