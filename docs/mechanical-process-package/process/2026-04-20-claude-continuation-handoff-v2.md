# Claude Continuation Handoff v2

Use `/ultraplan` first, then continue executing autonomously.

Do not stop at diagnosis.
Take over from the current state and keep building the hardness of the protocol and benchmark design until you either:

1. produce a materially harder protocol revision with locally verified artifacts, or
2. hit an explicit stop condition that proves the current design path should be abandoned.

## Mission

This is not issue churn.
This is not product work.
This is not a request for another optimistic verdict artifact.

Your job is to continue hardening the scientific-validation machinery for `#208`.

## Exact current state

- protocol branch: `issue-208-validation-protocol`
- protocol head: `7377bac`
- subject branch: `issue-208-scientific-validation-post-protocol`
- subject registration head: `6b36d16`
- exact logic head used for the local scientific run: `867d824ffdbe3063fb2bf2eb9993e6077427269d`

## What is already true

- a protocol-owned benchmark layer exists
- a protocol-owned benchmark runner exists
- a first post-protocol replay subject exists
- local B4-B7 benchmark execution passed on the exact detached logic head
- external adjudication still says `NOT_ESTABLISHED`

## Authoritative external result

Treat this as authoritative:

- Claude: `NOT_ESTABLISHED`
- Gemini: `NOT_ESTABLISHED`
- GLM: `NOT_ESTABLISHED`

Do not try to overrule that with more local evidence.

## Read first

- `docs/mechanical-process-package/process/2026-04-20-validation-protocol-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-future-scientific-validation-entry-gates-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-benchmark-manifest-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-adjudication-rules-v1.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-results-v2.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-adjudication-results-v2.toml`
- `docs/mechanical-process-package/process/2026-04-20-scientific-validation-adjudication-status-v2.toml`
- `docs/mechanical-process-package/validation/subjects/2026-04-20-post-protocol-subject-registration-v1.toml`
- `docs/mechanical-process-package/validation/benchmarks/`

## Main failure themes you must address

1. Subject eligibility is not scientifically credible.
The replayed subject is still seen as old logic wearing a new timestamp.

2. Corpus independence is still not convincing.
Even protocol-owned fixtures/runners are being judged as shaped around the harness's own rejection paths.

3. Protocol identity is muddy.
The protocol document, subject registration, and adjudicated snapshots are still too easy to challenge on exact-snapshot grounds.

4. Result framing must stay fail-closed.
Local execution artifacts must never smuggle in adjudicative truth values.

## What to do

1. Use `/ultraplan` to produce the next hardening plan.
2. Then execute that plan autonomously.
3. Prefer the smallest changes that materially reduce one of the blocker themes above.
4. Keep all work mechanical: artifacts, templates, gates, runners, fixtures, exact heads.
5. Commit each real protocol step separately.
6. Verify before each commit.

## What not to do

- do not start another unrelated issue
- do not keep layering ad hoc issue slices
- do not treat local passes as equivalent to establishment
- do not create another self-graded success artifact
- do not run external review yourself unless explicitly asked

## Desired end state

You do not need to establish scientific validation.

You do need to leave the repo in a better state such that:

- the next external adjudication target is sharper
- the subject eligibility rule is harder to fake
- the corpus independence rule is harder to dispute
- exact protocol/subject/result snapshot identity is harder to challenge

## Required output after `/ultraplan`

Then continue execution.

Only stop when you have either:

1. completed the next hardening slice and committed it, or
2. reached a concrete stop condition that says the protocol must be redesigned from scratch.
