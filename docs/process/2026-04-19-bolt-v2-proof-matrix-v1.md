# Bolt-v2 Proof Matrix v1

## Purpose

This document is the first frozen repo-level process artifact for `bolt-v2`.

Goal:
- stop discovering review criteria one PR at a time
- replace free-form "looks good" review with predeclared proof obligations
- make external review an audit of the obligation matrix, not the primary discovery engine

This is inspired by the Goedel prover pattern:
- decompose the task first
- prove sub-obligations, not only the whole change
- use verifier/compiler/runtime feedback as the correction loop

For `bolt-v2`, the analogue is:
- classify the change
- enumerate the hazards for that class
- require one proof artifact per hazard
- block "ready" unless every row is satisfied

## Freeze Rule

This matrix is frozen at `v1` for the evaluation below.

Derivation set:
- `#109`
- `#165`
- `#175`
- `#180`
- `#184`
- `#185`
- `#193`
- `#196`

Holdout set:
- `#201` (`#195`)
- `#202` (proof closure after merged `#198`)
- `#167`

The holdout set is used only to evaluate whether this frozen matrix would have blocked the miss before review.

## Readiness Rule

A PR is not "ready" if any required hazard row for its declared change class is missing a proof artifact.

Allowed terminal states for each row:
- `PROVED`
- `DISPROVED`
- `OUT OF SCOPE`

Anything else means the PR is not ready.

## Change Classes

### C1. CI / Cache / Workflow State

Typical examples:
- `#193`
- `#195` / `#201`
- `#196`
- `#184` / `#185` when the failure is workflow / gate coupling

Mandatory hazards and proof artifacts:

| Hazard | Required proof artifact |
| --- | --- |
| exact-head issue acceptance | exact-head CI/job/log lines showing the issue-level claim, not only green CI |
| restore path correctness | exact log lines proving the restore step used the intended path |
| save path correctness | exact log lines proving post-job persistence happened, not only capture |
| next-run persistence | two-run proof on same source state showing state written in run N is present in run N+1 |
| optimization degrades safely | one forced-failure proof for restore/capture helper showing the main test/build lane still executes |
| corrupt-state fallback | direct regression for malformed cache/manifest input |
| syscall / permission failure | direct regression for IO failure (`utime`, write, subprocess) |
| dirty checkout misuse | direct regression showing local unstaged changes are not normalized |
| no-op fast path | direct regression showing redundant writes are skipped |
| exact merge-ref wiring | proof on the actual PR merge shape, not only the branch-local workflow file |
| control-plane separation | evidence that unrelated required lanes are not being used as acceptance proof for the issue |

Block condition:
- any stateful optimization PR in this class is blocked unless all rows above are satisfied

### C2. Selector / Discovery / Admission

Typical examples:
- `#175`
- `#180`
- merged `#187` / `#198`
- `#188`

Mandatory hazards and proof artifacts:

| Hazard | Required proof artifact |
| --- | --- |
| canonical grouping | regression proving identical groups dedupe correctly |
| transitive overlap merge | regression proving A overlaps B, B overlaps C merges into one canonical fetch |
| non-overlap separation | regression proving disjoint windows remain separate |
| exact boundary semantics | regression at the exact inclusion/exclusion boundary, not only +/-1 |
| stale snapshot aging | regression proving old selector state ages out |
| failed-refresh persistence | regression proving failed groups preserve or age out exactly as declared |
| empty-success clearing | regression proving successful empty refresh clears stale state |
| fail-closed overflow | direct proof for overflow in both query construction and partition-time filtering |
| runtime admission contract | regression showing the runtime/hot path uses the intended narrowed state, not a parallel broad path |
| WS / refresh trade-off is explicit | proof or explicit out-of-scope statement for immediate admission vs periodic refresh |

Block condition:
- if any branch of grouping, aging, or fail-closed behavior is only "believed" rather than directly tested, the PR is blocked

### C3. Config / Validation / Fail-Closed Parsing

Typical examples:
- `#109`
- portions of `#175`

Mandatory hazards and proof artifacts:

| Hazard | Required proof artifact |
| --- | --- |
| hidden hardcodes | grep/code proof that the seam no longer depends on asset-specific or venue-specific literals |
| malformed input | regression proving invalid input halts or rejects before runtime |
| structural comparison | regression proving structured/canonical compare replaces raw-string equality |
| unknown-field rejection | regression proving config schema rejects drift fields |
| mixed-mode rejection | regression proving forbidden config combinations halt early |
| semantic-preservation proof | regression proving previously supported valid cases still pass |
| new-family proof | regression proving a new member of the same pattern works without new literals |

Block condition:
- if the change claims generalization but lacks one "new family / same pattern" proof, the PR is blocked

### C4. Strategy Semantics / External Truth Source

Typical examples:
- `#165`
- `#167`

Mandatory hazards and proof artifacts:

| Hazard | Required proof artifact |
| --- | --- |
| exact external truth source | authoritative source is identified and machine-consumed or explicitly proven unavailable |
| display vs resolution semantics | explicit proof whether UI value and settlement value are the same or distinct |
| reconstructed-value equivalence | if reconstructing, proof that the reconstructed value matches the authoritative value exactly |
| semantic freeze before code | no code claiming the behavior is correct before the semantics document is frozen |
| rollback / lifecycle failure path | direct regression on failure rollback paths, not only happy path |
| operator-safety fail-closed | regression proving uncertainty halts instead of trading on approximation |

Block condition:
- if the strategy depends on an external semantic anchor and that anchor is not proven exact, the PR is blocked

### C5. Scope / Decomposition Discipline

Typical examples:
- `#165`
- prior `#109` failure pattern

Mandatory hazards and proof artifacts:

| Hazard | Required proof artifact |
| --- | --- |
| one issue per PR | exact issue/PR scope map with declared out-of-scope items |
| slice legitimacy | proof that the slice can be validated independently |
| accepted residual tracking | every real residual is mapped to a tracked follow-up, not left in comments |
| exact-head review target | review packet includes exact head SHA and exact acceptance claims |
| no hidden second issue | diff and PR body agree on the real scope |

Block condition:
- if the PR cannot be judged independently of later work, it is blocked

### C6. Control Plane / Trust Root

Typical examples:
- `#184`
- `#185`

Mandatory hazards and proof artifacts:

| Hazard | Required proof artifact |
| --- | --- |
| main-vs-branch coupling | proof that the gate validates `main` or policy source of truth, not arbitrary feature-branch local state |
| branch coexistence | proof that unrelated feature branches can still commit/run tests |
| policy drift detection | explicit failure mode when policy and protected state disagree |
| fail-closed but non-self-blocking | regression proving the gate blocks real drift without blocking legitimate branch work |

Block condition:
- if a trust/root gate can self-block unrelated feature work, it is blocked

## Holdout Evaluation

### Holdout A: `#201` / issue `#195`

Class:
- `C1. CI / Cache / Workflow State`

Observed miss:
- the PR was close to "ready" even though the exact-head acceptance was still false
- later review found that exact-head warm-rerun proof was still missing / invalid
- then a workflow-drift bug caused the branch to use the wrong target-dir output wiring on the actual PR merge shape

Would matrix v1 have blocked it?
- Yes.

Why:
- missing `next-run persistence` proof would block it
- missing `exact-head issue acceptance` proof would block it
- missing `exact merge-ref wiring` proof would block it
- missing `optimization degrades safely` proof would block it

Verdict:
- `BLOCKED BY MATRIX`

### Holdout B: `#202`

Class:
- `C2. Selector / Discovery / Admission`

Observed miss:
- merged `#198` behavior was probably right, but four important branches were under-proven:
  - transitive overlap merge
  - exact expiry boundary
  - query-construction overflow
  - refresh-task invalid-interval guard

Would matrix v1 have blocked it?
- Yes.

Why:
- `transitive overlap merge` row was missing
- `exact boundary semantics` row was missing
- `fail-closed overflow` row was only partially satisfied

Verdict:
- `BLOCKED BY MATRIX`

### Holdout C: `#167`

Class:
- `C4. Strategy Semantics / External Truth Source`

Observed miss:
- strategy anchor was reconstructed from local runtime observations rather than proven exact against the market's settlement semantics

Would matrix v1 have blocked it?
- Yes.

Why:
- missing `exact external truth source` proof
- missing `display vs resolution semantics` proof
- missing `reconstructed-value equivalence` proof

Verdict:
- `BLOCKED BY MATRIX`

## Evaluation Result

The hypothesis survives this cheap historical replay:

- on all three held-out cases, matrix `v1` would have blocked the PR/state before external review
- each block comes from predeclared rows, not reviewer creativity

What this does **not** prove:
- that matrix `v1` is complete for all future `bolt-v2` changes
- that no new hazard class exists outside the current derivation set

What it does prove:
- the repo-specific matrix is materially better than the current vanilla-AI-plus-review flow
- the next process step should be matrix hardening, not more free-form PR experimentation

## Next Matrix Upgrade Rules

If future external review finds a blocker:
- if the blocker matches an existing row, the execution failed
- if the blocker falls outside all rows, the matrix failed and must be revised

No future blocker should be treated as "just another comment."
It must be classified as either:
- `MATRIX EXECUTION FAILURE`
- `MATRIX COVERAGE FAILURE`
