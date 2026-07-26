# Implementation Plan: Thin NautilusTrader Boundary for Current Decision Evidence

**Issue**: [#1354](https://github.com/seungpyoson/bolt-v2/issues/1354)
**Bolt PR**: [#1505](https://github.com/seungpyoson/bolt-v2/pull/1505)
**Required NT PR**: none. Bolt pins the exact official merged commit
`e4167fd1ed5ce9db06b43a81417ab4096b8b84b6`.
**Specification**: [spec.md](spec.md)
**Status**: Implementation in progress; verification and review gates pending

## Outcome

Delete Bolt's live order/fill/position reconciliation mirror. Capital admission
becomes one Bolt projection over:

- post-reconciliation NT state;
- committed Bolt action-authorization evidence;
- configured Bolt reservation policy; and
- provider-only collateral allowance.

The implementation must reduce authority and code. It must not add a framework,
fallback, or compatibility lane.

## Dependency Shape

This work is one reviewable change:

1. **Bolt PR #1505**: pin an exact official NT commit, delete Bolt's
   compensating reconciliation, and consume only post-reconciliation NT state.

Adapter completeness remains NT-owned and is not addressed here. The upstream
change that would make unmapped open orders, relevant confirmed fills, and
unrepresentable positions fail reconciliation is not merged, so at this pin
adapter completeness is an accepted open gap rather than a delivered guarantee.
Bolt does not add a compensating reconciliation layer to cover it.

## Dependency-Ordered Implementation

### Phase 1 — Pin the official NT boundary

- Pin Bolt and BTE to an exact official, merged NT commit through the official
  NT repository URL.
- Update the governed source-capability revision.
- Declare no source capability the pinned revision does not actually provide;
  adapter reconciliation completeness is not declared at this pin.

Evidence:

- full NT Polymarket package tests and all-target clippy;
- Bolt/BTE locked dependency resolution;
- Bolt build guard accepting the official URL and exact revision.

### Phase 2 — Make every projection post-reconciliation

- Preserve one projection-request flag shared by provider and NT callbacks.
- NT order, position, account, and portfolio callbacks set the flag only.
- Provider snapshots revoke readiness and set the same flag.
- The NT runtime watchdog consumes the flag only in `NodeState::Running`.
- The watchdog reads NT cache and performs the complete projection on the NT
  thread.

Evidence:

- request remains pending while `Starting`;
- requests coalesce;
- one projection runs after transition to `Running`;
- provider worker never reads NT cache or constructs reconciled authority.

### Phase 3 — Delete duplicate NT authority

Delete production code and tests for:

- Bolt live-order attribution maps used as lifecycle state;
- client/venue-order lifecycle maps;
- terminal-order history used to decide existence;
- event-derived fill/liability mutation;
- event-derived position deltas;
- source/timestamp lifecycle arbitration;
- raw provider open-order and position attestation;
- dormant pre-run provider query APIs for orders, positions, balances, and
  allowances;
- Bolt venue causal reconciliation and divergence;
- incremental venue/NT universe merging.

Keep only:

- current NT snapshot extraction;
- pure Bolt reservation/evidence join;
- evidence-specific deduplication that does not decide NT state; and
- typed fail-closed admission/health results.

Evidence:

- behavior suites over immutable projection inputs;
- structural review of the production call graph;
- no source-scanning test.

### Phase 4 — Reduce provider input to allowance

- Return `ProviderCollateralAllowanceSnapshot` directly from the registered Polymarket
  source.
- Keep only the provider allowance; discard the provider-reported balance from
  the combined REST response because NT owns balances.
- Delete the unused pre-run collateral-accounting query surface.
- Remove the duplicate raw provider snapshot transport.
- Delete the file-based allowance source and its config fields.
- Require the registered live provider whenever capital admission is enforced.
- Route every provider update through the same projection request.

Evidence:

- provider builder/runtime tests;
- enforced-config rejection without a registered source;
- no live alternative source or fallback.

### Phase 5 — Preserve Bolt evidence and recovery

- Keep committed admission attribution as Bolt's authorization record.
- Derive current liability from NT open orders joined to that attribution.
- Fail closed on unattributed NT orders.
- Treat attribution without an NT order as inert.
- Keep fill facts as Bolt audit/dedup evidence, not lifecycle mutation.
- Preserve current settlement replay and evidence durability behavior.
- Keep BTE on the same Bolt fact/codec/policy types.

Evidence:

- attributed/unattributed/orphan relation tests;
- restart equivalence tests;
- current-evidence contract and semantic-negative corpus;
- settlement, poison, cap, ownership, and BTE regressions.

### Phase 6 — Reconcile active documentation

- Update this specification, plan, external review request, and resolution.
- Mark the superseded money-loop documents as historical designs that must not
  be implemented.
- Update active runtime/runbook text that assigns lifecycle reconciliation to
  Bolt.
- State the two-PR dependency and exact official NT pin.

Evidence:

- targeted text checks;
- internal adversarial review of active claims.

### Phase 7 — Verify, review, and publish

1. Run focused tests for the changed authority boundaries.
2. Regenerate decision-evidence artifacts and prove deterministic output.
3. Run formatting and diff checks.
4. Run the advisory workflow's root and BTE clippy/test/build commands.
5. Conduct a class-complete internal adversarial review.
6. Resolve every substantive local finding.
7. Commit and push the exact Bolt head.
8. Obtain terminal-green exact-head advisory evidence.
9. Send the frozen review request to Claude, GPT, and Kimi.
10. Resolve external findings before requesting native code-owner review.

## Requirement-to-Evidence Matrix

| Requirement | Evidence |
|---|---|
| NT owns reconciliation | Bolt call-graph review |
| Complete NT reconciliation universe | NOT delivered at this pin; open gap recorded in spec.md section 1 |
| Admission-safe NT config | table test for every narrowing option |
| Post-reconciliation projection | `Starting`/`Running` request timing test |
| Events are triggers only | unchanged snapshot + callback permutations |
| No Bolt shadow lifecycle | structural call-graph review plus behavior tests |
| Provider allowance only | typed provider-source tests |
| No provider fallback | config rejection and single registry path |
| Exact evidence join | attributed/unattributed/orphan tests |
| Restart equivalence | uninterrupted/restart differential tests |
| Same live/BTE contract | shared types plus both workspace suites |
| No evidence regression | generated contract, codec, poison, cap, settlement tests |
| No adjacent #1385 work | three-dot scope review |

## Stop Conditions

Stop and revise the design if:

- Bolt must interpret general order lifecycle independently of NT;
- correctness requires a Bolt venue-order query or acknowledgement journal;
- a provider worker must read NT cache;
- a second allowance or reconciliation path appears necessary;
- a pre-`Running` path can consume projection work;
- BTE requires a different Bolt policy/reducer;
- a substantive internal or external finding remains unresolved; or
- the change begins implementing #1385.

## Completion Gate

The slice is ready for external review only when:

- Bolt compiles against its exact official-repository commit;
- every spec requirement has named evidence;
- all focused and full root/BTE verification passes;
- internal adversarial review has no unresolved substantive finding;
- active docs describe the implemented boundary;
- the Bolt worktree is committed, clean, and pushed; and
- no fallback, compatibility path, TODO, or duplicate NT authority remains.
