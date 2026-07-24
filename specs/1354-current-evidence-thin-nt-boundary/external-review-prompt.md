# External Architecture Review Request: PR #1505 Thin NT Boundary

Conduct a fresh, read-only architecture review of the specification package for
PR #1505's #1354 current-decision-evidence slice.

This review occurs before further production implementation. Review the design,
not merely the current code symptoms.

## Review Identity

Repository:
`https://github.com/seungpyoson/bolt-v2`

Pull request:
`https://github.com/seungpyoson/bolt-v2/pull/1505`

Base:
`d7a79229e7593f5a81940f30405db3f0dc2166a1`

At invocation, record and report:

- the exact review head;
- confirmation that the worktree or archive is clean;
- the exact merge base; and
- the specification files reviewed.

Do not modify files, branches, reviews, comments, the PR, or other GitHub state.

## Governing Inputs

Read `AGENTS.md` first. Then review:

- `specs/1354-current-evidence-thin-nt-boundary/spec.md`
- `specs/1354-current-evidence-thin-nt-boundary/plan.md`
- `specs/1354-current-evidence-thin-nt-boundary/external-review-resolution.md`

Use current production code only to verify feasibility and identify conflicting
authority:

- `src/bolt_v3_capital_admission_runtime_feed.rs`
- `src/bolt_v3_submit_admission.rs`
- `src/bolt_v3_live_node.rs`
- `src/bolt_v3_capital_admission.rs`
- `src/bolt_v3_capital_admission_state.rs`
- current decision-evidence recovery consumers
- the pinned NautilusTrader order/cache/reconciliation surfaces

The current pinned NT execution engine appears to update its cached order before
publishing the corresponding order event. Verify that ordering and its fill,
position, account, reconciliation, and message-bus siblings rather than
assuming it applies uniformly.

## Problem Being Resolved

NT is supposed to own orders, fills, positions, accounts, lifecycle,
reconciliation, adapters, and venue translation.

Bolt currently also maintains a live-order set, client/venue mapping,
terminal-event history, order-lifecycle snapshot, and event-derived position
changes for capital admission. Repeated reviews found temporal and
reconciliation defects because that state partially duplicates NT.

The proposed correction is intentionally thinner:

- NT canonical state is the sole live lifecycle authority.
- Bolt owns only its durable authorization evidence, reservation policy, and
  admission decision.
- Venue truth attests NT reconciliation completeness and provides only facts
  absent from NT; it is not a second lifecycle authority.
- Raw callbacks trigger reprojection rather than mutate a Bolt lifecycle model.
- Any failed join or authority disagreement exposes zero new-risk capability.

## Hard Constraints

- NO HARDCODES, NO DUAL PATHS, and NO DEBTS.
- Do not rebuild NT-owned order, fill, position, account, adapter, or
  reconciliation behavior.
- Do not add a Bolt event or acknowledgement journal.
- Do not make venue truth a parallel order-lifecycle authority.
- Do not add a compatibility decoder, fallback, alternate reducer, or runtime
  mode.
- Tests verify behavior, never source structure.
- Preserve current evidence append/sync, poison, recovery, cap, ownership, and
  hard-cutover guarantees unless a compatible signature change is necessary.
- Do not implement #1385 rotation, capacity, retirement, ordinals, or restart
  append-retry exact-once.
- External review is architecture evidence, not merge authority.

## Required Review Questions

1. Does the authority table leave NT as the sole owner of general order
   lifecycle and reconciliation?
2. Is the proposed Bolt projection genuinely thinner than the current
   event-mutated mirror, or does it recreate NT state under another name?
3. Can the implementation obtain a canonical NT snapshot after reconciliation
   and at live trigger boundaries using existing pinned NT surfaces?
4. Can raw callbacks be treated as triggers without missing a risk-changing
   transition or observing NT cache before NT applies the event?
5. Is venue truth limited to completeness attestation and provider-only facts?
6. Can every venue/NT/evidence disagreement prevent new-risk capability without
   requiring a second lifecycle authority?
7. Is deriving reservation liability from canonical NT state plus committed
   Bolt evidence sufficient for partial fills, terminal orders, unknown external
   activity, and restart?
8. Does the transition table cover duplicate, delayed, missing, reordered, and
   contradictory inputs without source/timestamp precedence?
9. Is restart reconstruction equivalent to uninterrupted execution without
   process-local callback history?
10. Do live and BTE share one projection without creating a mock-only alternate
    authority?
11. Can provider-thread inputs revoke admission and reach an existing NT-thread
    dispatch surface without reading the thread-confined NT cache, reopening
    admission off-thread, or creating a second journal?
12. Does any required correction introduce a fallback, compatibility lane,
    duplicated state, verifier ecosystem, or #1385 work?
13. What invariant class, authority boundary, or lifecycle transition remains
    unmodeled?

## Finding Requirements

For every substantive finding provide:

- severity: Critical, High, Medium, or Low;
- violated requirement or transition row;
- exact file/section and current-code evidence where applicable;
- a concrete failure sequence;
- root cause;
- sibling sweep performed;
- the smallest systematic correction consistent with the hard constraints; and
- behavior/integration proof required to close the class.

Do not:

- produce stylistic findings;
- recommend per-event or per-purpose wrapper proliferation;
- recommend source-scanning tests;
- treat tracking or documentation as resolution of a current correctness gap;
- defer current correctness to #1385;
- assume an empty NT cache proves venue emptiness; or
- recommend that Bolt independently adjudicate general NT lifecycle.

## Required Output

End with:

### A. Verdict

Choose exactly one:

- `ACCEPT DESIGN`
- `REVISE DESIGN`

### B. Required changes

Give one dependency-ordered list. If no changes are required, state `None`.

### C. Rejected or overstated concerns

Identify proposed findings that are not defects and explain why.

### D. Missing-class challenge

Name one still-unreviewed authority or lifecycle boundary, or state with
specific evidence that the requested boundary set is complete.

Do not approve implementation merely because current CI is green. This review
is complete only when the specification is coherent and implementable as the
thinnest safe layer over NT.
