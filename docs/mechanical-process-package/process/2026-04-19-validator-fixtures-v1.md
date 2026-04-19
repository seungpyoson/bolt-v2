# Validator Fixtures v1

## Purpose

These are reference fixtures for the future mechanical validator implementation.

They define what the validator should report on the current experiment artifacts.

## Fixture 1: ETH Anchor Semantics

Input:

- `docs/delivery/exp-eth-anchor-semantics/`

Expected result:

- fail

Expected failures:

```text
STATUS: BLOCK
KIND: semantic
WHERE: seam_contract.toml / seams[0]
WHY: storage_field `active.interval_open` allows multiple source classes and authoritative_source is not frozen
NEXT: freeze one authoritative anchor source and list forbidden fallback sources explicitly
```

```text
STATUS: BLOCK
KIND: evidence
WHERE: evidence_bundle.toml / E4 + E5
WHY: public market page exposes `priceToBeat` while sampled Gamma payload does not expose a matching direct anchor field
NEXT: freeze which external artifact is authoritative for the seam or block implementation
```

```text
STATUS: BLOCK
KIND: semantic
WHERE: seam_contract.toml / seams[1]
WHY: stale-chainlink seam does not declare one authoritative clock source
NEXT: declare the seam clock explicitly and update proof obligations for disagreement cases
```

## Fixture 2: Finding Canonicalization

Input:

- `docs/delivery/exp-finding-canonicalization/`

Expected result:

- pass

Expected summary:

```text
STATUS: PASS
KIND: finding
WHERE: finding_ledger.toml
WHY: repeated slug-fetch wording collapsed into one canonical finding; stale NT-pointer comments classified as review_target_mismatch; legacy schema-boundary finding remained distinct
NEXT: none
```

## Fixture 3: Proof Plan Adequacy

Input:

- `docs/delivery/exp-proof-plan-selector-path/`

Expected result:

- pass with analytical conclusion

Expected summary:

```text
STATUS: PASS
KIND: proof
WHERE: proof_plan.toml
WHY: schema-boundary, legacy-compatibility, fail-closed legacy behavior, and bounded slug-fetch behavior are all represented as explicit claims with falsifiers
NEXT: none
```

Expected analytical note:

```text
STATUS: WARN
KIND: review_target
WHERE: process decomposition
WHY: stale review-target artifacts are not owned by proof_plan and must be filtered by review_target gate instead
NEXT: enforce review_target.toml whenever review-derived findings are present
```

## Acceptance Use

A future validator implementation should be considered acceptable only if it can reproduce these fixture outcomes from the current artifact package without relying on free-form interpretation.
