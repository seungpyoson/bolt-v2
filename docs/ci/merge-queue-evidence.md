# Merge Queue Evidence

## Authority Boundary

The GitHub ruleset supplies merge authority through native code-owner approval, stale-review dismissal, last-push approval, and review-thread resolution. Mergify supplies routing and a single-PR queue. CI workflows are advisory evidence and are not required-status, Mergify-predicate, or preflight admission inputs.

The admission details live in [Merge Queue Preflight Contract](merge-queue-preflight-contract.md). This document describes evidence capture without restating that contract.

## Operator Evidence

For a queue operation, retain:

- the expected base SHA and PR head SHA used by `just merge-queue`;
- the preflight verdict and selected queue rule;
- the PR URL and Mergify queue comment or resulting queue item;
- direct confirmation of the live GitHub rules governing `main` when merge authority is under review.

Queue only one PR per invocation. Only `queue_as_one_wave` grants queue permission; every other result refuses queueing.

## Advisory CI Evidence

Capture CI only when it supports a claim or risk relevant to the change. Record the exact PR head, workflow/run identity, result, and what requirement the result supports. A green result cannot replace native review; a red, cancelled, skipped, missing, or unavailable result is evidence to adjudicate, not an automatic admission verdict.

Use `just verify-remote` for applicable exact-head Rust evidence and Rust Probe only for scoped diagnosis. Follow [Ubicloud Runner Cost Governance](ubicloud-cost-governance.md) for runner selection and cost controls.

## Mismatch Handling

If repository configuration, the live ruleset, Mergify behavior, or observed evidence disagrees with these contracts, stop the queue operation and report the mismatch. Do not add a fallback admission path or reintroduce CI predicates to compensate for external-state drift.
