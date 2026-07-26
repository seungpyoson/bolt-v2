# External Review Request: PR #1505 Thin NT Boundary

SUPERSEDED: this prompt was written while Bolt pinned unmerged NT work and
still asks reviewers to review an NT dependency that is no longer part of the
change. Bolt now pins the exact official merged commit
`e4167fd1ed5ce9db06b43a81417ab4096b8b84b6` and requires no NT PR.

Conduct one fresh, read-only, class-complete architecture and implementation
review of Bolt PR #1505.

This is a closure review, not another open-ended point-finding round. Review
the selected authority boundary across both changes, sweep siblings for every
finding, and state whether the implementation is ready for native review.

## Review Identity

Bolt repository: `https://github.com/seungpyoson/bolt-v2`
Bolt PR: `https://github.com/seungpyoson/bolt-v2/pull/1505`
Bolt base: `40423b291683effe645bde44edce91be8ef93000`
Bolt exact head: `<INSERT_PUSHED_EXACT_HEAD>`
NT exact commit: `e4167fd1ed5ce9db06b43a81417ab4096b8b84b6`

At review start, verify and report:

- live Bolt PR head equals the requested exact head;
- Bolt merge base equals the requested base;
- NT PR head contains the requested exact commit;
- review snapshot/worktree is clean; and
- exact-head advisory status.

Read only. Do not modify files, branches, comments, reviews, PRs, or other
GitHub state.

## Governing Inputs

Read Bolt `AGENTS.md` first. Then read:

- `specs/1354-current-evidence-thin-nt-boundary/spec.md`
- `specs/1354-current-evidence-thin-nt-boundary/plan.md`
- `specs/1354-current-evidence-thin-nt-boundary/external-review-resolution.md`
- the exact Bolt three-dot diff and current production call graph;
- NT PR #4566's exact diff and every Polymarket order, fill, position, and
  mass-status reconciliation caller.

Inspect at least:

- `src/bolt_v3_capital_admission_runtime_feed.rs`
- `src/bolt_v3_capital_admission.rs`
- `src/bolt_v3_capital_admission_state.rs`
- `src/bolt_v3_submit_admission.rs`
- `src/bolt_v3_live_node.rs`
- `src/bolt_v3_providers/polymarket.rs`
- `src/bolt_v3_providers/polymarket/provider_collateral_allowance_runtime_source.rs`
- current decision-evidence facts, recorder, consumers, and contract
- Bolt and BTE Cargo manifests/locks and NT source-capability binding
- NT `crates/adapters/polymarket/src/execution/reconciliation.rs`

## Selected Architecture

NT is the sole owner of adapter protocols, venue reconciliation, orders, fills,
positions, accounts, portfolio, and cache lifecycle.

Bolt owns only configuration/registration, strategy intent, shared admission
policy, committed action-authorization evidence, and provider facts NT does not
represent.

The implementation must have this shape:

1. NT Polymarket reconciliation fails if any venue open order or relevant
   confirmed fill cannot map into NT's instrument universe, or if any current
   position cannot be represented. The shared order/fill builders make this
   failure mandatory for every caller.
2. Bolt requires an admission-safe NT reconciliation configuration.
3. Bolt reads one ephemeral NT snapshot on the NT runtime thread, only after
   `NodeState::Running`.
4. NT lifecycle callbacks may record Bolt-owned audit evidence, but lifecycle
   changes only request a fresh projection; provider updates revoke readiness
   and request that same projection.
5. Provider input contains collateral allowance only.
6. Bolt joins NT open orders to committed Bolt admission attribution and fails
   closed on any missing relation.
7. No Bolt live-order set, fill reducer, position reducer, venue-order
   attestation, or causal reconciliation remains.
8. Live and BTE share one Bolt admission/evidence contract.

## Hard Constraints

- NO HARDCODES, NO DUAL PATHS, NO DEBTS.
- Prefer deletion and existing NT surfaces.
- Do not recommend a Bolt venue-order query or reconciliation engine.
- Do not add an acknowledgement journal, compatibility decoder, fallback,
  alternate provider source, runtime mode, or second reducer.
- Tests verify behavior, not source tokens.
- Preserve current evidence durability, poisoning, finite caps, single-writer
  ownership, producer capabilities, settlement recovery, and hard-cutover
  semantics.
- Do not implement #1385 rotation, retained capacity, retirement, durable
  ordinals, or restart append-retry exact-once.
- External review is evidence, not merge authority.

## Required Review Classes

Adjudicate every class below:

1. **NT authority** — Can any Bolt production state still decide general order,
   fill, position, account, or reconciliation state independently of NT?
2. **NT reconciliation completeness** — Does NT #4566 close silent partial
   open-order, relevant confirmed-fill, and current-position reconciliation,
   including every builder caller and error-propagation path?
3. **Configuration closure** — Can any configured NT filter/lookback setting
   still make the admission universe incomplete?
4. **Temporal/thread boundary** — Can projection run before reconciliation,
   off the NT thread, or against a cache state older than the triggering event?
5. **Provider boundary** — Does provider input contain only facts absent from
   NT, with one live source and no file/fallback lane?
6. **Authorization join** — Can an unattributed NT order, orphan evidence,
   duplicate relation, partial fill, or terminal order create incorrect
   liability or new-risk capability?
7. **Recovery equivalence** — Does restart reconstruct the same Bolt state from
   NT + committed evidence + provider collateral allowance without callback history?
8. **Evidence integrity** — Did the deletions preserve contract closure,
   append/sync ordering, poison behavior, caps, ownership, and consumer
   validation?
9. **Concurrency/lifecycle** — Can callbacks, health emission, shutdown, or
   projection requests deadlock, race readiness, lose a required reprojection,
   or outlive their authority?
10. **BTE parity** — Does BTE reuse the same Bolt types/rules without pretending
    to be live NT or creating a mock-only authority?
11. **Dependency/governance** — Is Bolt pinned to one exact official NT source,
    with no fork fallback or unregistered source fact?
12. **Scope** — Is any present correctness defect incorrectly deferred to the
    runbook or #1385, or is adjacent work hidden in this slice?

## Finding Requirements

For every substantive finding provide:

- severity;
- invariant class;
- exact file, symbol, and line evidence;
- concrete failure sequence;
- sibling sweep performed;
- root cause;
- smallest systematic correction consistent with the hard constraints;
- whether it blocks native-review readiness; and
- proof required to close the whole class.

Explicitly distinguish:

- verified present defects;
- rejected or overstated concerns;
- deterministic verification failures;
- environment limitations;
- accepted hard-cutover losses;
- operational live-cutover prerequisites; and
- genuine #1385 work.

Do not:

- manufacture style findings;
- repeat a known issue without checking the exact head;
- recommend per-purpose wrapper or verifier proliferation;
- recommend source-scanning tests;
- treat tracking/documentation as fixing present correctness;
- infer venue emptiness from an empty NT cache; or
- move NT-owned lifecycle logic back into Bolt.

## Required Output

End with:

### A. Verdict

Choose exactly one:

- `READY FOR NATIVE REVIEW`
- `CHANGES REQUIRED`

### B. Required implementation sequence

Give one dependency-ordered list, or `None`.

### C. Architecture statement

State plainly whether Bolt is now the thinnest safe layer over NT and identify
any remaining duplicated authority.

### D. Missing-class challenge

Name one concrete still-unreviewed lifecycle/authority boundary, or state with
specific evidence that the requested class set is complete. Do not manufacture
a speculative boundary merely to satisfy this section.
