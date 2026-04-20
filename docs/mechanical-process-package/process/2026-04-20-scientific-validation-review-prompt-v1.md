# Scientific Validation Review Prompt v1

You are doing an external adjudication of a frozen scientific-validation run.

This is NOT a product PR review.
This is NOT a prototype adjudication.
This is a review of whether scientific validation is established for the bounded claim set on the exact frozen subject and protocol below.

## Exact Subject

- Repo: `seungpyoson/bolt-v2`
- Subject branch: `issue-208-scientific-validation-subject`
- Subject head SHA: `50c0fca5c21ea4a4b94ce1136ab68aed0ab3e105`

## Exact Protocol

- Protocol branch: `issue-208-validation-protocol`
- Protocol head SHA: `8b69e99e75f68b393532f38b03435b7e88b5d741`

## Primary artifacts to judge

- `docs/mechanical-process-package/process/2026-04-20-validation-protocol-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-benchmark-manifest-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-adjudication-rules-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-results-v1.toml`
- `docs/mechanical-process-package/validation/benchmarks/B4-unsupported-comparator-kinds.toml`
- `docs/mechanical-process-package/validation/benchmarks/B5-scalar-summary-schema-breaks.toml`
- `docs/mechanical-process-package/validation/benchmarks/B6-summary-replay-drift.toml`
- `docs/mechanical-process-package/validation/benchmarks/B7-producer-contract-schema-breaks.toml`
- `src/delivery_validator.rs`
- `src/summary_replay.rs`
- `tests/delivery_validator_cli.rs`

## Frozen context

- The protocol branch was created after the prototype branch was frozen.
- This run is meant to test the bounded fault families B4–B7 only.
- Reported local execution on the subject head:
  - `delivery_validator_cli`: 53 passing tests
  - `summary_replay`: 4 passing unit tests
  - benchmark families B4–B7 executed and recorded as passing
- Do not trust those claims automatically. Verify if needed.

## What you are judging

1. Is the scientific-validation protocol specific enough?
2. Is the held-out benchmark corpus frozen and non-redefinable enough?
3. Are the recorded benchmark results mechanically adequate?
4. On this exact subject head and protocol head, is scientific validation established for the bounded claim set?

## Strict rules

- Judge ONLY this exact subject head and protocol head.
- Do not drift into unrelated product issues or prototype adjudication.
- Do not reward prose quality.
- Do not suggest future improvements unless they are necessary to justify the current verdict.
- If something is not proven, say so plainly.
- Findings first. No long overview first.
- Collapse duplicates.

## Output format

OVERALL
- verdict: ESTABLISHED | NOT_ESTABLISHED | INCONCLUSIVE
- protocol_specific_enough: yes | no
- benchmark_corpus_frozen_enough: yes | no
- benchmark_results_adequate: yes | no
- evidence_overstated: yes | no

FINDINGS
- finding 1
  - kind:
  - locus:
  - why:
  - proof:
  - severity: critical | major | minor
- finding 2
  ...

If there are no live findings, say exactly:
- No live findings on exact subject head 50c0fca5c21ea4a4b94ce1136ab68aed0ab3e105 and protocol head 8b69e99e75f68b393532f38b03435b7e88b5d741.

Final requirement
- End with one binary sentence:
  - `Recommendation: scientific validation established for the bounded claim set.`
  or
  - `Recommendation: scientific validation not established.`
  or
  - `Recommendation: inconclusive.`
