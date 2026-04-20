# Scientific Validation Review Prompt v2

You are doing an external adjudication of a frozen scientific-validation run.

This is NOT a product PR review.
This is NOT a prototype adjudication.
This is a review of whether scientific validation is established for the bounded claim set on the exact frozen subject and protocol below.

## Exact Subject

- Repo: `seungpyoson/bolt-v2`
- Subject branch: `issue-208-scientific-validation-post-protocol`
- Subject logic head SHA: `867d824ffdbe3063fb2bf2eb9993e6077427269d`
- Subject registration head SHA: `6b36d1627f3e1db72d7b9beeff6b7f0dcddf4b30`

## Exact Protocol

- Protocol branch: `issue-208-validation-protocol`
- Protocol head SHA: `6ed9711403a482a8698cd7a22721dd0c0c06e2b1`

## Primary artifacts to judge

- `docs/mechanical-process-package/process/2026-04-20-validation-protocol-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-future-scientific-validation-entry-gates-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-benchmark-manifest-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-adjudication-rules-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-results-v2.toml`
- `docs/mechanical-process-package/validation/subjects/2026-04-20-post-protocol-subject-registration-v1.toml`
- `docs/mechanical-process-package/validation/benchmarks/B4-unsupported-comparator-kinds.toml`
- `docs/mechanical-process-package/validation/benchmarks/B5-scalar-summary-schema-breaks.toml`
- `docs/mechanical-process-package/validation/benchmarks/B6-summary-replay-drift.toml`
- `docs/mechanical-process-package/validation/benchmarks/B7-producer-contract-schema-breaks.toml`
- `docs/mechanical-process-package/validation/benchmarks/fixture-sets/review-package-protocol-owned.toml`
- `docs/mechanical-process-package/validation/benchmarks/runners/held-out-b4-b7-review-runner.toml`
- `src/delivery_validator.rs`
- `src/summary_replay.rs`
- `src/bin/process_validator.rs`

## Frozen context

- The subject logic was replayed after the protocol freeze onto a fresh branch from `main`.
- The benchmark layer is protocol-owned and does not cite subject-authored tests as held-out evidence.
- Reported local execution on the exact subject logic head:
  - `delivery_validator_cli`: 53 passing tests
  - `summary_replay`: 4 passing unit tests
  - benchmark families B4–B7 executed and recorded as passing through `scientific_validation_runner`
- Do not trust those claims automatically. Verify if needed.

## What you are judging

1. Is the subject eligible under the frozen protocol and entry gates?
2. Is the benchmark corpus frozen and independent enough?
3. Are the recorded benchmark results mechanically adequate?
4. On this exact subject and protocol, is scientific validation established for the bounded claim set?

## Strict rules

- Judge ONLY this exact subject and protocol snapshot.
- Do not drift into unrelated product issues or prototype adjudication.
- Do not reward prose quality.
- Do not suggest future improvements unless they are necessary to justify the current verdict.
- If something is not proven, say so plainly.
- Findings first. No long overview first.
- Collapse duplicates.

## Output format

OVERALL
- verdict: ESTABLISHED | NOT_ESTABLISHED | INCONCLUSIVE
- subject_eligible_under_protocol: yes | no
- benchmark_corpus_frozen_enough: yes | no
- benchmark_execution_adequate: yes | no
- evidence_overstated: yes | no
- self_graded_residue: yes | no

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
- No live findings on exact subject logic head 867d824ffdbe3063fb2bf2eb9993e6077427269d and protocol head 6ed9711403a482a8698cd7a22721dd0c0c06e2b1.

Final requirement
- End with one binary sentence:
  - `Recommendation: scientific validation established for the bounded claim set.`
  or
  - `Recommendation: scientific validation not established.`
  or
  - `Recommendation: inconclusive.`
