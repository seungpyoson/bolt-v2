# Claim Enforcement Replay Experiment v1

## Hypothesis H13

`claim_enforcement_coverage.toml` should stop being a trusted intermediate file and become mechanically reproducible evidence.

The validator should recompute it from:

- `merge_claims.toml`
- `claim_enforcement.toml`

and fail on drift.

## Validation Date

- 2026-04-20

## Treatment

1. Add deterministic recomputation of `claim_enforcement_coverage.toml`.
2. Compare the checked artifact against the recomputed output.
3. Keep the gate language unchanged:
   - it still consumes the scalar artifact
   - replay happens outside the gate layer

## Evidence Set

Targeted H13 falsification corpus:

- 1 review fail fixture:
  - `claim_enforcement_coverage` count fields drift from recomputed output
- 1 merge-candidate fail fixture:
  - `claim_enforcement_coverage.summary_verdict` drifts from recomputed output

Broader regression evidence:

- 49 `delivery_validator_cli` tests passed
- candidate `#205` review package still passes
- H3-H12 surfaces stayed green
- all-stage gate matrix still passes

## "Statistically Significant" Metaphor

What is proven here:

1. the scalar summary is no longer trusted on appearance alone
2. the validator now checks producer replay, not just schema
3. direct drift falsifiers fail cleanly
4. the wider regression surface stayed green after replay was introduced

## Result

H13 passes.

`claim_enforcement_coverage.toml` is now mechanically reproducible evidence, not just a trusted scalar file.

## Remaining Boundary

The next replay target is:

- `orchestration_reachability_summary.toml`

That would complete the current scalar producer family replay loop.
