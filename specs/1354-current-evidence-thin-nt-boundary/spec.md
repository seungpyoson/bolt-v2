# Feature Specification: Thin NautilusTrader Boundary for Current Decision Evidence

**Issue**: [#1354](https://github.com/seungpyoson/bolt-v2/issues/1354)
**Bolt PR**: [#1505](https://github.com/seungpyoson/bolt-v2/pull/1505)
**Required NT PR**: none. Bolt pins the exact official merged commit
`e4167fd1ed5ce9db06b43a81417ab4096b8b84b6`, so this slice depends on no unmerged
upstream work. See "Complete reconciliation is not an NT guarantee at this pin".
**Branch**: `codex/1354-current-evidence-rebuild`
**Status**: Implemented; internal adversarial review and exact-head verification pending

## Purpose

PR #1505 introduces current-only, typed, durable decision evidence. Its first
capital-admission implementation also recreated part of NautilusTrader's order,
fill, position, and reconciliation state inside Bolt. That was the wrong
boundary.

NT owns adapter protocols, execution, orders, fills, positions, accounts,
portfolio state, cache state, and reconciliation. Bolt owns only:

- TOML/SSM composition;
- strategy intent and Bolt admission policy;
- the decision evidence proving Bolt authorized an action;
- reservation policy derived from current NT state; and
- one provider fact NT does not represent, currently collateral allowance.

This specification deletes the Bolt shadow lifecycle. It introduces no
compatibility path, alternate venue reconciliation, or second order ledger.

## Governing Invariant

> NT is the sole live lifecycle authority. Bolt may construct new-risk
> capability only from a post-reconciliation NT snapshot joined to committed
> Bolt authorization evidence and current provider-only allowance. Missing
> or unavailable input leaves admission unreconciled.

## Authority Contract

| Concern | Sole authority | Bolt may | Bolt must not |
|---|---|---|---|
| Adapter protocol and venue translation | NT | Configure and call NT | Reimplement provider execution semantics |
| Order existence/status | NT cache | Read a post-reconciliation snapshot | Keep a parallel live-order set |
| Fill/remaining quantity | NT order and fill state | Derive current reservation liability | Rebuild progress from callback history |
| Positions/accounts/portfolio | NT | Read a canonical snapshot | Maintain a competing position or balance reducer |
| Venue reconciliation | NT | Require successful reconciliation; completeness is unverifiable at this pin (section 1) | Query venue orders and reconcile them again |
| Polymarket collateral allowance | Provider REST fact | Supply the approval allowance missing from NT | Supply balances or carry order, fill, or position lifecycle |
| Action authorization | Bolt decision evidence | Prove Bolt admitted an NT order | Infer authorization from NT state alone |
| Reservation/admission policy | Bolt | Join attributed NT orders to configured policy | Redefine NT lifecycle |

## Selected Architecture

### 1. Complete reconciliation is not an NT guarantee at this pin

The Polymarket adapter returns successful partial reconciliation reports when
venue open orders or relevant confirmed fills reference instruments absent from
NT's loaded map. It can also omit an unrepresentable current position. At the
pinned revision those cases are logged and skipped rather than raised
(`crates/adapters/polymarket/src/execution/reconciliation.rs`), so a mass-status
report can be silently partial. That cannot be repaired safely in Bolt without
rebuilding NT reconciliation, which this slice explicitly does not do.

This is an accepted open gap, not a solved problem. Bolt pins the exact official
merged commit `e4167fd1ed5ce9db06b43a81417ab4096b8b84b6` and therefore cannot
distinguish a complete report from a partial one. The upstream change that would
close it is not merged and is deliberately outside this slice; until it lands,
the reconciliation completeness the authority contract above depends on is a
requirement Bolt states but cannot verify. Whether shipping on that basis is
acceptable is a review decision, not an implementation detail.

When capital admission is enforced, Bolt also requires an admission-safe NT
configuration:

- reconciliation enabled;
- unbounded reconciliation lookback;
- no reconciliation instrument filter;
- no client-order filter;
- no unclaimed-order filter;
- no position-report filter; and
- missing-order generation enabled;
- ongoing open-order reconciliation enabled with an unbounded lookback; and
- ongoing position reconciliation enabled.

### 2. One post-reconciliation projection

Bolt computes admission from ephemeral values read on the NT runtime thread
after `NodeState::Running`:

- configured account;
- NT open orders and their current quantities;
- NT yes/no positions;
- NT account/portfolio values used by existing policy;
- committed Bolt admission attribution;
- current provider collateral allowance; and
- configured Bolt capital policy.

The result is either a complete projection or a typed unreconciled state.
Only the complete result can expose new-risk capability.

Bolt retains the resulting admission state it owns. It does not retain NT
orders, fills, positions, terminal histories, or source/timestamp arbitration
as competing lifecycle authority.

### 3. Events request projection; they do not mutate lifecycle

NT order, fill, position, account, and portfolio callbacks only request a fresh
projection. Provider allowance updates do the same.

Every trigger converges on one `AtomicBool` request:

1. the producer publishes its owned fact;
2. provider failure/update immediately revokes readiness where required;
3. the NT runtime watchdog observes the request;
4. it waits until NT is `Running`;
5. it reads the current NT cache on the NT thread; and
6. it rebuilds the whole Bolt admission projection.

No callback payload adds/removes an order or applies a fill to Bolt lifecycle
state.

### 4. Provider input is allowance only

Polymarket's provider worker returns one typed `ProviderCollateralAllowanceSnapshot`.
It contains only the allowance, source identity, account/venue/currency
identity, and observation time needed by Bolt policy. The provider-reported
balance from the combined REST response is deliberately discarded; NT remains
the balance authority.

There is one production source: the registered Polymarket runtime provider.
The previous file-based source is deleted. The provider input contains no open
orders, positions, client/venue mappings, or causal reconciliation data.
The unused pre-run provider query APIs for orders, positions, balances, and
allowances are deleted; they cannot become a second lifecycle or account
authority later.

### 5. Evidence remains Bolt-owned

Bolt records the action authorization and operational audit facts it owns.
Current NT open orders must join to committed admission attribution.

- NT order with matching committed attribution: eligible for projection.
- NT order without committed attribution: admission stays unreconciled.
- committed attribution without an NT open order: inert for current liability.
- fill audit without predecessor attribution: machine evidence fails closed.

Evidence deduplication may prevent duplicate Bolt facts. It must not decide
whether an NT order exists or is live.

### 6. Restart uses the same authorities

Restart waits for complete NT reconciliation, then rebuilds from:

1. the canonical NT snapshot;
2. committed current-only Bolt evidence;
3. Bolt policy; and
4. a fresh provider collateral allowance snapshot.

No process-local callback history is required. BTE supplies NT-equivalent
snapshot inputs to the same Bolt policy/evidence types; it does not emulate the
live provider or create a second reducer.

## Required Failure Behavior

| Condition | Required result |
|---|---|
| NT startup reconciliation incomplete or failed | No new-risk capability |
| NT adapter cannot map a venue open order | NT reconciliation fails; Bolt never projects |
| NT adapter cannot map a relevant confirmed fill | NT reconciliation fails; Bolt never projects |
| Admission-unsafe reconciliation config | Configuration rejected |
| Projection requested before NT is `Running` | Request remains pending |
| NT open order lacks committed Bolt attribution | Typed unreconciled result |
| Provider allowance missing, stale, or failed | New-risk admission unavailable |
| Provider update arrives off the NT thread | Revoke/request only; never read NT cache |
| Duplicate/reordered callbacks | Same result as the same canonical NT snapshot |
| Bolt attribution has no NT order | Inert; no invented reservation |
| Restart | Fresh full projection; no inherited process-local lifecycle |
| Machine evidence corruption | Activation fails closed |
| Observation evidence corruption | Recording poisoned; machine authority unchanged |

## Functional Requirements

- **FR-001 — NT ownership**: NT MUST be the only live authority for adapters,
  venue reconciliation, orders, fills, positions, accounts, portfolio, and
  cache lifecycle.
- **FR-002 — No Bolt shadow OMS**: Bolt MUST NOT maintain a live-order set,
  client/venue lifecycle map, terminal-order ledger, fill-progress reducer, or
  event-derived position authority.
- **FR-003 — Upstream completeness**: Polymarket reconciliation MUST fail if
  any venue open order or relevant confirmed fill is absent from NT's
  instrument universe, or any current position cannot be represented.
- **FR-004 — Safe NT configuration**: enforced capital admission MUST reject
  every reconciliation setting that narrows the startup or ongoing universe,
  disables ongoing open-order or position checks, or bounds the ongoing
  open-order lookback.
- **FR-005 — Post-reconciliation boundary**: canonical NT cache reads and
  admission projection MUST occur on the NT runtime thread only after NT is
  `Running`.
- **FR-006 — Trigger-only callbacks**: NT and provider callbacks MAY request
  projection and record owned evidence, but MUST NOT mutate Bolt lifecycle
  mirrors.
- **FR-007 — One provider fact**: live provider input MUST be the registered
  allowance snapshot. No file fallback or raw venue-order attestation may
  exist.
- **FR-008 — Exact authorization join**: every admission-relevant NT open
  order MUST have one valid committed Bolt attribution.
- **FR-009 — Fail closed**: unavailable reconciliation, provider input, or
  evidence relation MUST expose zero new-risk capability.
- **FR-010 — Fresh recovery**: readiness may return only after a fresh complete
  projection; no cached success, patch, or latest-source arbitration.
- **FR-011 — Restart equivalence**: process-local callback history MUST NOT be
  necessary for restart reconstruction.
- **FR-012 — Same Bolt contract**: live and BTE MUST use the same admission
  policy, evidence facts, codecs, and consumer projections.
- **FR-013 — Existing evidence guarantees**: atomic admission, sync-before-
  receipt, poisoning, finite caps, single-writer ownership, typed producer
  capabilities, and hard-cutover behavior MUST remain intact.
- **FR-014 — Deletion**: replaced lifecycle/reconciliation code MUST be
  deleted, not retained behind a feature, mode, fallback, or compatibility
  adapter.
- **FR-015 — Scope**: rotation, retained capacity, retirement, durable
  ordinals, and restart append-retry exact-once remain in #1385.

## Explicit Non-Goals

- Reimplementing NT reconciliation in Bolt.
- Adding a Bolt venue acknowledgement journal.
- Persisting provider snapshots as lifecycle authority.
- Adding another allowance source.
- Changing NT submission or cancellation mechanics.
- Adding historical decision-evidence decoding.
- Implementing #1385.
- Authorizing live cutover.

## Required Evidence

Tests verify behavior, not source text.

- Upstream NT tests: one or more unmapped open orders or relevant confirmed
  fills fail every applicable reconciliation caller; an unrepresentable
  current position also fails mass-status reconciliation.
- Bolt config table: every universe-narrowing NT reconciliation option fails
  closed when capital admission is enforced.
- Runtime timing test: projection requests coalesce, remain pending while NT is
  `Starting`, and run once when NT is `Running`.
- Provider test: the runtime source produces only typed allowance input.
- Admission tests: attributed NT orders reconstruct liability; unattributed
  orders fail closed; orphan evidence is inert.
- Callback tests: duplicate and reordered triggers do not change the result for
  an unchanged NT snapshot.
- Restart tests: uninterrupted and restarted projections agree for the same
  authoritative inputs.
- Existing evidence, poison, settlement, shutdown, cap, generated-contract,
  root, and BTE suites remain green.
- Exact-head advisory formatting, production clippy, tests, and release builds
  are terminal green before native review.

## Success Criteria

- No Bolt production type decides live order, fill, position, or reconciliation
  state independently of NT.
- No provider input contains NT-owned lifecycle data.
- No provider input carries an alternate balance value.
- No file/fallback allowance path remains.
- No pre-`Running` path can consume a projection request or reopen admission.
- Every NT open order used by admission joins to committed Bolt evidence.
- The pinned NT adapter cannot silently return a partial open-order, confirmed
  fill, or current-position universe.
- Live and BTE use one Bolt policy/evidence contract.
- Internal adversarial review has no unresolved substantive finding.
- External architecture review has no unresolved Critical, High, or Medium
  finding.

## Scope Relations

This is the thin-NT-boundary closure required for PR #1505's current-only
decision-evidence slice. Bolt consumes NT through an exact official-repository
pin at a merged commit. The NT adapter-completeness correction is out of scope
for this slice and is not a dependency of it.

Issue #1385 retains rotation, total retained capacity, retirement, durable
ordinals, and restart append-retry exact-once. Live-cutover quiescence,
archival, exact-artifact proof, and operator authorization remain separate
operational prerequisites.
