# Merge Queue Preflight Contract

## Purpose

`just merge-queue <pr>` is a narrow admission helper for the Mergify queue. It verifies immutable input identity and the existing pull-request/native-review mechanics needed to route one PR. It does not execute CI, aggregate CI verdicts, or duplicate GitHub ruleset authority.

## Admission Boundary

An authoritative run verifies:

- the operator supplied the expected base SHA and selected PR head SHA;
- the fetched base and PR head match those expected identities;
- the PR is open, non-draft, targets the configured base, and is mergeable;
- GitHub reports the existing required-reviewer approval state;
- the `.mergify.yml` blob in the synthetic integration commit is valid and routes the PR to exactly one supported queue rule;
- each queue rule requires only the configured reviewer and has `batch_size: 1`;
- the PR merges cleanly with the expected base.

Native code-owner approval, stale-review dismissal, last-push approval, and review-thread resolution remain authoritative in the GitHub ruleset. Preflight must not add parallel implementations of those controls.

## Explicit Exclusions

Queue admission must not:

- poll `gh pr checks`, classify required-check state, or wait for CI;
- inspect workflow-to-check maps or Mergify `check-success` predicates;
- execute source-fence, Cargo, `just`, or ad hoc verifier commands;
- aggregate advisory workflow results into an admission verdict;
- build multi-PR batches or add a compatibility path for repositories that still expose required checks.

CI and source-fence output can support engineering claims, but they are evidence outside this admission boundary.

## Source Of Truth

- Runtime values come from `ci/rust-verification.toml`.
- Queue shape and routing come from the `.mergify.yml` blob in the synthetic integration commit, never from the local worktree, base alone, or PR head alone.
- This candidate-state invariant permits a queue-policy change to establish its successor contract without requiring the predecessor configuration to satisfy that successor contract first.
- Required-reviewer identity and expected queue shape are mirrored by `scripts/ci_provenance.py` and checked by workflow-hygiene tests.
- The live GitHub ruleset is the source of truth for native review controls.

## Results

The machine-readable and plain-text outputs use these verdicts:

- `queue_as_one_wave` / exit `0`: one PR satisfies the admission contract;
- `blocked` / exit `2`: the PR fails a definitive identity, state, approval, or mergeability requirement;
- `inconclusive` / exit `3`: required metadata or inspection evidence is unavailable or ambiguous;
- exit `4`: invalid input or internal tool failure.

`--no-gh` is diagnostic only and cannot authorize queue submission. The operator posts the configured Mergify command only after `queue_as_one_wave`.

## Residual Risk

Preflight reports risks it does not eliminate: base/head or metadata drift after inspection, later queue/config changes, live queue ordering, reset after an external merge, and queue-check cost. None of those disclosures turns CI into admission authority.

## Verification

The focused tests cover identity mismatch, PR state and mergeability, required-reviewer approval, queue routing, single-PR sizing, base conflicts, unavailable metadata, invalid input, and the invariant that green, failed, missing, skipped, cancelled, or unavailable CI evidence produces the same admission decision.
