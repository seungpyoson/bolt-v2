# External Review Resolution: PR #1505 Thin NT Boundary

**Status**: Internal review and exact-head verification pending
**Review scope**: [spec.md](spec.md), [plan.md](plan.md), Bolt PR #1505, and
[NT PR #4557](https://github.com/nautechsystems/nautilus_trader/pull/4557)
**Protocol**: [external-review-prompt.md](external-review-prompt.md)

This is the single adjudication record for the next external review. It grants
no merge, deploy, readiness, or trading authority.

## Frozen Bundle

Complete only after local findings are resolved and the Bolt head is pushed:

- Bolt exact head: pending
- Bolt base/merge base:
  `40423b291683effe645bde44edce91be8ef93000`
- NT exact commit:
  `9c755a109185216444bdd4618ba52d9c583f5d13`
- clean-worktree confirmation: pending
- exact-head advisory result: pending
- specification hashes: pending

## Architecture Decision

The earlier design that made Bolt reconcile raw provider orders, fills, and
positions against NT is superseded.

Selected boundary:

- NT owns all general lifecycle and reconciliation.
- NT #4557 makes incomplete Polymarket open-order, relevant confirmed-fill,
  and current-position reconciliation errors, declares complete adapter
  reconciliation capabilities, and fences risk-increasing execution after
  strict reconciliation authority is lost.
- Bolt consumes only post-reconciliation NT state.
- Bolt requires strict, unbounded, unfiltered NT reconciliation for every live
  configuration and treats position-close callbacks as projection triggers,
  not lifecycle authority.
- provider input supplies collateral allowance only.
- Bolt events are audit facts or projection triggers, never lifecycle
  authority.
- no file provider, raw-order/position query API, pre-run collateral query,
  raw-order attestation, or compatibility path remains.

## Reviewers

All reviewers receive the identical frozen prompt and exact heads.

| Reviewer | Verdict | Unique substantive findings |
|---|---|---|
| Claude | pending | pending |
| GPT | pending | pending |
| Kimi | pending | pending |

## Adjudication Rules

Record every unique substantive finding once:

- **Accepted**: implementation/specification change required.
- **Rejected**: exact evidence proves it is not a defect.
- **Duplicate**: same root cause and systematic correction as another finding.
- **Out of scope**: owned by a named issue and does not hide current
  correctness.

“Tracked,” “documented,” or “CI is green” is not resolution.

Every accepted finding records its invariant, failure sequence, root cause,
systematic correction, proof, commit, and re-review result. Every rejected
finding records concrete Bolt or NT evidence.

## Finding Register

No external findings have been received for the frozen implementation.

## Acceptance Gate

Native review may be requested only when:

- local internal review has no unresolved substantive finding;
- the exact Bolt head is clean, committed, and pushed;
- exact-head advisory evidence is terminal green;
- all reviewers used the same frozen bundle;
- no substantiated Critical, High, or Medium external finding remains;
- every accepted finding is implemented and re-reviewed; and
- the final call graph contains one NT lifecycle authority, one provider
  allowance source, and no fallback or #1385 implementation.
