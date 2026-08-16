# Submit Admission Authority Boundary Feasibility Decision

Status: **not feasible at the pinned production boundaries**.

This decision grants no live-submit, deployment, merge, migration, recovery,
implementation, venue-probe, or NautilusTrader-change authority.

## Decision

Do not derive or implement a production submit-authority replacement from the currently
pinned Bolt and NautilusTrader surfaces.

This document answers only whether those surfaces presently contain enough trustworthy
evidence to justify such work. They do not. It is a negative feasibility decision, not a
future architecture, certification framework, state machine, implementation plan, or
cutover procedure.

### Why negative evidence stops production work

Production work requires positive evidence for every necessary authority property. One
confirmed unresolved property is enough to stop that work. This decision records four such
properties; it does not prove that no possible architecture could resolve them.

This document therefore does not claim that its production inventory or unresolved-property
list is complete. Discovering another mutation path, retry, evidence gap, race, or recovery
product which lacks a trustworthy owner strengthens the denial. Newly discovered
capability-bearing evidence can instead invalidate the affected gap and requires a fresh
evaluation. It does not grant permission by itself, and resolving one necessary property does
not grant authority while another remains unresolved.

No later artifact inherits permission from this decision. A future artifact must use fresh
pins and direct evidence, and must obtain its own explicitly scoped approval. This decision
remains the historical verdict for the pins below rather than being superseded into a
future implementation contract.

## Pins and evaluated question

This decision is pinned to:

- Bolt `e62584045629208e81d2dce1fce608720ea01fbf`; and
- NautilusTrader `e4167fd1ed5ce9db06b43a81417ab4096b8b84b6`.

The evaluated question is:

> Can Bolt replace its current direct-order admission owner with one production
> submit-authority boundary, using the pinned production surfaces, without adding a dual
> mutation path, reconstructing NT-owned protocol state, or inferring permission from
> incomplete evidence?

The answer is **no**.

For this decision, a production replacement must at minimum:

- preserve ordinary and maker live submission plus kill-switch forced reduction;
- remain safe and operational across repeated submissions, outcome uncertainty, and
  process restart;
- conserve resources while an effect is unresolved and eventually release them when
  completion is proved; and
- replace the current owners with one production mutation boundary without reconstructing
  NT-owned adapter or lifecycle state.

A component which rejects every submit, permanently stops after the first uncertain
outcome, never releases proved-complete resources, or cannot restart is fail-closed but is
not a production replacement.

The facts below are the verified evidence needed for that answer. They are not presented
as an exhaustive authority-surface manifest.

## Verified production facts

### Bolt has multiple mutation and admission surfaces

At the Bolt pin:

- ordinary Live submission enters
  `BoltV3OrderExecutionPolicy::route_submit_with_sink`, calls admission, invokes an NT
  venue-mutation sink, and then calls `commit_submitted`;
- maker submission has a distinct production runtime which calls
  `Strategy::submit_order`;
- Shadow executes the stateful
  `evaluate_and_record_without_consuming_capacity` path without invoking a venue sink;
- kill-switch forced reduction uses `BoltV3NtSubmitOnlySink` with a closure that manually
  adds the order to NT cache, publishes `OrderInitialized`, constructs `SubmitOrder`, and
  executes the risk engine;
- live cancel and cancel-all routes reach NT directly, including from maker commands;
- live in-place modify is refused before its sink call, while generic mutation sinks still
  implement modify methods; and
- basket admission is instantiated only in tests, although its dormant module retains
  reservation machinery.

These paths do not currently form one production authority boundary. This fact does not
select a future owner or prescribe how a cutover should work.

### One NT submit call does not identify one physical effect

At the NT pin, `Strategy::submit_order` can:

1. deny an active market exit and return `Ok(())` without routing;
2. add the order to cache and publish `OrderInitialized` before routing;
3. route to the emulator, an execution algorithm, or the risk engine;
4. receive a downstream risk denial without venue contact; and
5. run `set_gtd_expiry` after routing, which can return `Err` and can immediately invoke a
   nested `cancel_order` when the order is already expired.

The resulting `SubmitOrder` carries client, position, execution-client, command, and parameter
context beyond the order itself. Its schema defines optional correlation and causation fields,
but this `Strategy::submit_order` constructor populates neither. The call's return value cannot
prove whether a venue mutation did not occur, did occur, or was followed by another mutation.

### One Polymarket logical operation can contain multiple physical attempts

At the NT pin:

- market and limit submissions use the adapter retry manager;
- a retry can follow a transport result whose submit outcome is unknown;
- both paths derive the expected venue order identity before retry;
- an internal flag upgrades a terminal failure after any unknown attempt, but an eventual
  success does not expose that earlier unknown-attempt history;
- cancel operations also use adapter retry; and
- the `POST /order` body carries the signed order, API owner, order type, and post-only
  choice, but no account-capacity reservation, source revision, conditional authority, or
  venue-enforced reduce-only condition.

Neither a local return nor a single logical-call result closes the physical-attempt set.

### Trade evidence is emitted before irreversible economic finality

At the NT pin:

- the WebSocket adapter checks for unknown maker-leg instruments before fan-out, then
  reloads the instrument map and can still skip an unknown leg inside the emission loop;
- the complete source transaction survives only in opaque `info`;
- MATCHED fills can be emitted before settlement finality;
- a later FAILED trade status emits cumulative `OrderFillVoided` evidence only for fills still
  recoverable from bounded dispatch or fill-tracker state; direct MATCHED-fill records use a
  10,000-entry FIFO and can be evicted before FAILED arrives;
- voided quantities and commissions are cumulative for the referenced trade;
- the correction identity is adapter-generated and its causation UUID refers to a parent
  fill event rather than a venue-native total correction revision;
- an NT order can transition from Canceled to Filled; and
- a fill void can reopen or otherwise change effective economics.

Current status plus the currently observed trade set therefore closes only an observed
prefix. It does not prove that economically effective successors cannot arrive.

### Source and report surfaces do not provide an authoritative restart cut

At the pins:

- status-report filled quantity is caller supplied;
- mass-status insertion can overwrite reports by venue-order key;
- no Bolt-facing producer binds an authenticated snapshot root, snapshot revision,
  applied-through revision, current source head, and live-subscription start; and
- no durable source-specific floor proves that a restart cut descends from the last
  authority-bearing cut rather than from an older, internally consistent snapshot.

The pinned surfaces therefore cannot establish a gap-free, non-rollback source projection
from which positive authority can be reconstructed after restart.

## Unresolved necessary properties

Each property below is necessary for the production replacement defined above. The pinned
evidence does not demonstrate all of them, so production work is not justified. These
properties form a combined no-go case; unless a section explicitly says otherwise, it is not
a proof-independent impossibility claim or a claim that every possible architecture has been
ruled out.

### 1. Physical-effect closure is not demonstrated

Safe closure could come from authenticated per-attempt or aggregate disposition, proved
idempotence or coalescence, or a bounded conservative attempt envelope closed by
authoritative observation. The reviewed production profile configures a finite retry count,
and the adapter derives an expected venue identity, but the reviewed evidence does not prove
any end-to-end closure alternative: success can erase earlier unknown-attempt history, nested
mutations remain possible, and the authoritative observation needed to close an envelope is
itself absent below. The current evidence therefore does not justify safe retry, resource
release, tombstone completion, or restart reconstruction. This property is not claimed to be
independent of properties 3 and 4.

### 2. Mutation admissibility is not demonstrated

No demonstrated venue operation atomically enforces the shared aggregate risk
postcondition for the exact order across all writers and concurrent attempts. No complete
conservative theorem proves that postcondition from the available sources either.

The same gap exists separately for reduction authority: a caller label such as
`RiskReducingExit`, forced reduction, or reduce-only is a claim, not authenticated evidence
that every reachable fill outcome leaves risk no greater than before. The pinned
Polymarket request provides no venue-enforced reduce-only condition.

Consequently neither new-risk nor reduction permission has a trustworthy positive
constructor at these pins.

### 3. Irreversible economic release evidence is not demonstrated

The pinned surfaces permit fills before settlement finality, later voids, reopened
economics, and late status transitions. They expose neither venue-specific irreversible
finality nor a proved maximum late-change envelope. Full release of order, position, or
capital holds would therefore infer safety from the absence of a successor which can still
arrive.

### 4. Non-rollback source continuity is not demonstrated

The pinned surfaces expose no authenticated, gap-free cut which is also proved to descend
from the last durable authority-bearing source floor. A stale but internally consistent
snapshot can therefore not be distinguished from an acceptable restart projection using
the available Bolt-facing evidence.

Positive submit or release authority cannot be reconstructed safely after restart from
these surfaces.

## Consequences for current work

- Do not continue the rejected submit-admission implementation as a sequence of local
  invariant patches.
- Do not create a production state machine, executable authority model, implementation
  plan, dormant consumer, compatibility path, or cutover from this decision.
- Do not infer that an NT return, local lock, caller classification, current status, or
  current snapshot closes any blocker above.
- Do not change NautilusTrader or probe a live venue under this document.
- Keep the pinned production code authoritative until a separately authorized task reaches
  a different, independently reviewed decision.

This is a completed feasibility outcome, not deferred implementation debt.

## Re-entry rule

Future work requires a new, explicitly user-approved scope at fresh exact pins. That work
must begin from concrete producer and consumer behavior which directly resolves every
unresolved necessary property above. It may also discover additional gaps.

The future artifact must make its own architecture, evidence, review, and cutover case. It
cannot cite this decision, an earlier approval, a proposed type name, or an unavailable
future API as positive evidence. Review and approval mechanics belong to that artifact's
review request and the then-current repository governance; they are not encoded here.

No venue interaction, upstream work, or production implementation is authorized merely to
satisfy this re-entry rule. Each such action requires its own explicit user authorization.

## Review boundary

Review of this artifact should determine:

1. whether each asserted production fact is accurate at the exact pins;
2. whether the unresolved necessary properties collectively support the negative feasibility
   conclusion, without relying on an unclaimed proof of independence; and
3. whether any sentence accidentally grants permission or specifies an unverified future
   authority path.

Every false or misleading factual assertion remains a substantive finding. An omitted
additional gap does not invalidate this negative decision because the document makes
no completeness or positive-sufficiency claim.

## Non-goals

- no future runtime enums, transition tables, certificate types, or proof framework;
- no production state machine, executable composition model, or implementation plan;
- no cutover, migration, fallback, compatibility path, or dual path;
- no claim that the verified facts enumerate every production authority surface;
- no claim about which architecture could eventually resolve the necessary properties;
- no live activation or venue probe; and
- no upstream NautilusTrader action.
