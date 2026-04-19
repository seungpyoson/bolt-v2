# ETH Anchor Semantics Experiment Result

## Hypothesis

If the mechanical delivery process is working, it should block ETH anchor work before implementation when the seam still permits ambiguous anchor semantics.

## Treatment

Applied the process artifacts to the seam only:

- `issue_contract.toml`
- `seam_contract.toml`
- `proof_plan.toml`
- `finding_ledger.toml`
- `evidence_bundle.toml`
- `merge_claims.toml`

No strategy code was changed.

## Evidence Used

- local writer/read path for `interval_open`
- local tests that accept fallback to fused `fair_value`
- local forced-flat stale-chainlink guard inputs
- local reference-actor staleness clock test
- one public Polymarket event page sample
- one public Gamma event payload sample for the same slug

## Result

The process blocked the seam before implementation.

Mechanical result:

- all experiment TOML artifacts parsed successfully
- `merge_ready = false`
- `open_blockers = 3`
- `open_findings = 3`

## Blockers Raised

1. `interval_open` still permits mixed anchor source classes.
2. Public page artifact exposes `priceToBeat`, while sampled Gamma event payload does not expose a matching direct anchor field.
3. The stale-chainlink seam does not yet freeze which clock is authoritative.

## Verdict

The experiment passed its first objective.

It did not prove the final anchor semantics.
It proved that the process can stop implementation early when the seam is still semantically ambiguous.

That is the exact behavior the old process failed to enforce.
