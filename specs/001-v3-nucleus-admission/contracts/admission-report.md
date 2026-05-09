# Contract: Admission Report

## Verdict Values

- `ADMITTED`: no blockers and scan universe proven.
- `BLOCKED`: blockers present.
- `UNSCANNABLE`: scan universe could not be proven.

## Blocker Record

Each blocker record must include:

```text
BLOCKER <id>
class: <blocker-class>
invariant: <invariant-id>
severity: blocker
evidence:
  - <path>:<line> :: <short excerpt>
retire_when: <condition>
```

For absence proof, evidence must include the searched roots and search terms:

```text
evidence:
  - absent :: searched=<roots> terms=<terms>
```

## Warning Record

Warnings use the same shape as blockers, but `severity` is `warning` and they
do not affect default or strict admission unless promoted by a blocker rule.

## Required Blocker Classes

- `generic-contract-leak`
- `missing-contract-surface`
- `unowned-runtime-default`
- `unfenced-concrete-fixture`
- `narrow-verifier-bypass`
- `scan-universe-unproven`

## Ordering

Records are sorted by:

1. blocker class order listed above;
2. blocker id;
3. path;
4. line number.

## Secret Safety

Evidence excerpts must redact values that look like API keys, private keys,
tokens, passphrases, or secret material. Paths to SSM parameters may be printed
because they are configuration identifiers, not secret values.
