# Claude Ultraplan Handoff v1

Use `/ultraplan`.

Your job is not to continue issue churn.
Your job is to decide the minimum redesign needed so this can become a real scientific-validation protocol instead of a self-referential one.

## Exact working state

- protocol branch: `issue-208-validation-protocol`
- protocol head: `34a2734`
- subject branch: `issue-208-scientific-validation-post-protocol`
- subject registration head: `6b36d16`
- exact logic head used for the local run: `867d824ffdbe3063fb2bf2eb9993e6077427269d`

## Exact current outcome

- local protocol-owned B4-B7 benchmark run exists and passed
- external review verdict is still `NOT_ESTABLISHED`
- do not assume the current protocol is salvageable as-is

## Primary artifacts

- `docs/mechanical-process-package/process/2026-04-20-validation-protocol-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-future-scientific-validation-entry-gates-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-benchmark-manifest-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-adjudication-rules-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-results-v2.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-adjudication-results-v2.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-adjudication-status-v2.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-review-prompt-v2.md`
- `docs/mechanical-process-package/validation/subjects/2026-04-20-post-protocol-subject-registration-v1.toml`
- `docs/mechanical-process-package/validation/benchmarks/`

## External adjudication result to treat as authoritative

- Claude: `NOT_ESTABLISHED`
- Gemini: `NOT_ESTABLISHED`
- GLM: `NOT_ESTABLISHED`

## Main blocker themes from external review

1. Subject eligibility is not scientifically credible.
The "post-protocol" subject is mostly replayed old logic, not genuinely protocol-constrained new logic.

2. Corpus independence is still disputed.
Even though the benchmark layer is protocol-owned, reviewers still see it as shaped around the harness's own rejection paths.

3. Result framing was too strong.
Local execution facts and adjudicative claims were too mixed.

4. Protocol subject identity is still muddy.
The protocol originally pinned an ineligible prototype head, while later artifacts evaluated a different branch/head.

## What to do in /ultraplan

Produce a concrete plan that answers only these:

1. What exact scientific claim is still worth trying to validate?
2. What exact subject construction rule would make the subject genuinely eligible?
3. What exact benchmark construction rule would make the corpus genuinely independent?
4. What exact artifact set is the smallest one that can support a non-disputable external verdict?
5. Should this protocol be repaired, narrowed drastically, or abandoned in favor of a different validation design?

## What not to do

- do not keep layering more ad hoc issue slices
- do not treat local benchmark passes as success
- do not write another optimistic verdict artifact
- do not assume the current entry gates are sufficient just because they are formalized

## Required output shape

Return:

1. one-page diagnosis
2. one binary recommendation: repair narrowly | redesign fundamentally | kill
3. one stepwise ultraplan with explicit stop conditions
