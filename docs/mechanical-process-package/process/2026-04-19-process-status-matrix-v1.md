# Process Status Matrix v1

## Proven

### P1. Seam lock can block semantic ambiguity before implementation

Evidence:

- [exp-eth-anchor-semantics](/Users/spson/Projects/Claude/bolt-v2/docs/delivery/exp-eth-anchor-semantics/README.md)
- `merge_ready = false`
- 3 explicit blockers

Meaning:

The process can stop work at the semantic boundary instead of waiting for review to discover the mismatch.

### P2. Finding canonicalization can collapse repeated wording into one canonical finding

Evidence:

- [exp-finding-canonicalization](/Users/spson/Projects/Claude/bolt-v2/docs/delivery/exp-finding-canonicalization/README.md)
- repeated `join_all` comments collapsed into one finding
- stale NT-pointer review artifacts classified as review-target mismatch

Meaning:

The process can reduce review-loop churn from wording drift and stale PR targets.

### P3. Proof-plan adequacy can be tested against real late blocker classes

Evidence:

- [exp-proof-plan-selector-path](/Users/spson/Projects/Claude/bolt-v2/docs/delivery/exp-proof-plan-selector-path/README.md)
- direct local tests passed for schema-boundary, legacy compatibility, fail-closed legacy behavior, and bounded concurrency proof surfaces

Meaning:

The process can name which blocker classes should have been forced before review.

### P4. Gate ownership can be decomposed mechanically

Evidence:

- [2026-04-19-blocker-class-to-gate-map-v1.md](/Users/spson/Projects/Claude/bolt-v2/docs/process/2026-04-19-blocker-class-to-gate-map-v1.md)

Meaning:

Not every blocker belongs to the same gate, and the process now names the owning gate explicitly.

## Disproved

### D1. Proof-plan alone is sufficient

Disproved by:

- stale review-target artifacts from rebuilt selector-path PRs

Meaning:

`review_target` is mandatory.

### D2. One generic review checklist can keep findings MECE

Disproved by:

- repeated wording drift
- stacked PR stale-diff artifacts
- different blocker classes mixing scope, semantics, and evidence

Meaning:

Canonical findings and explicit gate ownership are required.

### D3. Passing local or CI tests implies the issue was actually proved

Disproved by:

- ETH anchor seam ambiguity
- historical selector-path late blocker classes

Meaning:

Claims must map to proof obligations, not just green tests.

## Still Unproven

### U1. End-to-end fresh issue success

Not yet proven:

The process has not yet been run on a brand-new issue from intake through review where external review mostly confirms instead of discovering a new blocker class.

Current progress:

- first fresh candidate selected: `#205`
- intake completed
- seam/proof lock completed
- fail-closed design defaults frozen
- implementation plan drafted
- implementation completed on branch `issue-205-smoke-tag-proof`
- review-stage package bound to exact implementation head
- validator passes on the `#205` package at review stage
- external review and exact-head CI evidence still not folded into the package

### U2. Thin validator implementation

Not yet proven:

The validator has only been specified, not implemented.
We have a contract and fixtures, not working enforcement code.

### U3. Runtime-monitor layer

Not yet proven:

The post-merge/runtime-monitor part of the process has not been tested on a live or staging seam.

### U4. Cross-repo generalization

Not yet proven:

The current evidence is strongest on `bolt-v2`.
It has not been replayed against `claude-config` or `claude-system-config`.

## Current Best Reading

This process is no longer just prose.

It now has:

- a core architecture
- a validator contract
- validator fixtures
- three evidence-bearing experiments
- a gate-ownership model

But it is still pre-production, because the actual validator implementation and one full fresh-issue run remain unproven.
