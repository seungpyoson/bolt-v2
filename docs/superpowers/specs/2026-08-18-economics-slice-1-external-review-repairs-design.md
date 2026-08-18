# Economics Slice 1 External-Review Repair Design

**Date:** 2026-08-18

**Owning issue:** #1445

**Pull request:** #1544

**Reviewed head:** `524362b68ed86d7bc84f63655b8202590974dac9`

**Base:** `e62584045629208e81d2dce1fce608720ea01fbf`
**Status:** Approved direction; implementation requires approval of this written design

## Decision

Repair the current branch structurally. Do not close the external findings with
independent guards, catch-all branches, runtime assertions, or another mirrored
state machine.

The repair has three architectural objectives:

1. Make exposure recovery, replacement discharge, and projections derive from
   one typed authority model.
2. Make maker command identity and quote-transaction phase products explicit in
   types instead of correlated fields, numeric sentinels, or parallel variants.
3. Make economic provenance truthful: configuration cannot convert missing
   provider economics into provider-proven zero, and dormant execution authority
   cannot survive a declared route deletion.

## Scope and non-goals

This is a review-repair round for #1445. It owns the confirmed defects and
structural debt listed below, introduced or exposed by the final repair range
`623801311..524362b68`. It does not wire the deferred maker NT-event lifecycle
surface, arm live execution, implement economics Slices 2-5, or implement maker
pre-arming work from #869.

Issue #817 is closed. Its live continuation is #869. Before requesting another
review, #869 must carry a durable note explicitly naming the deferred maker
NT-event-to-lifecycle/event-fence reconciliation, and the stable PR body must
name that same remainder and tracker.

Historical forced-reduction evidence may remain readable. Historical decoding
is inert and is not execution authority. No current producer, admission intent,
economics scenario, or submit route may remain for forced reduction while the
loaded configuration rejects that route.

## Finding disposition

| External finding | Disposition | Design response |
| --- | --- | --- |
| Replacement conflict can discard a working entry | Confirmed, reachable P1 | One replacement-conflict discharge operation preserves `pending_entry` for every event ordering |
| Missing Polymarket fee descriptor becomes zero | Confirmed, reachable P1 and contradicts the PR body | Delete the assertion policy; missing descriptor always fails closed |
| Maker leg is not bound to its instrument | Confirmed by both reviews | Mint one sealed leg authority from the active market and validate the command and final order against it |
| Entry rejection reaches a latent `unreachable!` and mislabels evidence | Confirmed structural defect | Carry the decision generation and map the exact typed rejection; no panic path |
| Blind recovery reason/provenance pairs are runtime-validated | Confirmed latent defect | Replace the correlated pair with a cause enum whose variants own their required payloads |
| `+1 conditional` claim | Confirmed evidence error | Retract it and publish range-specific `if` and `match` counts |
| PR body omits takeover scope and accepted remainder | Confirmed governance defect | Update the stable body and the live #869 tracking record |
| Forced-reduction submit authority is dormant | Confirmed | Delete current execution/admission authority while retaining only inert historical decoding where required |
| Wind-down mirrors arm/sink/poison phases | Confirmed structural debt | Share one attempt phase across active and winding-down modes |
| Scope closure uses `now_ns: 0` | Confirmed hardcoded sentinel | Thread an actor-clock observation through terminal settlement and scope closure |
| Requote liability phase is enum-plus-number | Confirmed latent defect | Encode the cancel/resubmit phase as a variant |
| Terminal refinement contains an impossible `unreachable!` | Confirmed latent defect | Compute the disposition first and wrap it afterward |
| Six exposure projections use `_ => None` | Confirmed exhaustiveness gap | Derive all projections from one exhaustive projection function |
| Cancel failure settles as `CommandIssued` | Valid naming/model concern; proposed refund rejected | Rename the boundary to NT mutation invocation and retain conservative charging after that boundary |

## Exposure authority design

### One replacement-conflict discharge

`ReplacementConflictState` owns the only operation that can discharge a closed
retained episode. The operation receives fresh canonical projection truth and
returns the next state plus optional replacement-adoption evidence.

For canonical `None`, a closed retained episode becomes:

- `PendingEntry(retained.pending_entry)` when a working entry remains; or
- `Flat` only when no pending entry remains.

Both position-close ordering and later canonical-projection ordering invoke this
same operation. Neither reducer may independently choose `Flat`.

### Cause-shaped blind recovery

Replace `BlindRecoveryState { reason, provenance }` with one cause-shaped value:

- `Probe`: a probe-class reason and optional retained authority;
- `IdentityBearing`: an identity-class reason, recorded episode, and optional
  retained authority;
- `RestartAdoption`: a restart-class reason, instrument, a non-empty order-ID
  collection, and optional retained authority;
- `ForeignVenue`: instrument and both venue identities plus optional retained
  authority.

Authority presence is also typed as `AuthorityFree` or `Retained`, rather than
interpreted from `Option<Box<ExposureState>>`. Fresh canonical `None` can release
only `AuthorityFree`; `Retained` recovery requires its class-specific continuity
proof. Thus a probe-class cause does not silently discard authority merely
because the same reason is reachable from both flat and occupied origins.

The evidence reason is derived from the cause. Constructors accept only the
payload required by their variant. No production constructor uses `panic!`,
`assert!`, or a reason/provenance agreement check.

The non-empty restart order set is represented as a first order ID plus remaining
IDs, or an equivalent private non-empty collection. Empty restart adoption is
not representable.

### One exhaustive projection

One exhaustive `ExposureState` projection produces the retained views used by
entry, position, exit, recovery, and sink-unknown queries. The projection names
every `ExposureState` variant. Wrapper states such as blind recovery and
obligation saturation recursively project their retained authority through this
single seam.

The existing query methods become field accessors or small transformations of
that projection. They do not contain independent state matches or wildcard
arms. Adding an exposure variant therefore creates one compiler error at the
projection owner instead of silently returning `None` from six places.

### Decision-bound operation generation

One pure operation-classification function owns the state-to-eligibility map for
entry, exit, bootstrap, recovery, and correction. Decision evaluation reads its
typed classification and generation without temporarily minting and dropping a
grant. Grant construction invokes the same classifier and then arms that exact
generation.

Entry and exit decisions capture the exposure generation used to derive the
decision. The later route request uses that captured generation rather than a
fresh read. A state change between decision and routing yields a typed stale
rejection. The current exit-evaluation probe-grant/dropped-grant cycle is
deleted.

Entry rejection evidence distinguishes stale generation, an already-armed
operation, and occupied exposure. The caller records the mapped reason and
returns a non-routing result. It does not call a second occupancy predicate and
does not contain `unreachable!`.

## Polymarket fee authority

Delete `PolymarketAbsentFeeDescriptorPolicy` and the TOML
`absent_fee_descriptor_policy` field. `FeeDescriptorUnknown` always returns
`PolymarketEconomicsError::FeeDescriptorUnknown`, which maps to invalid
authoritative snapshot and fails admission closed.

`PointEstimate::ProvenZero` is emitted only when authoritative provider bytes
contain an explicit fee descriptor whose evaluated rate and formula produce
zero. Its source remains the provider snapshot because the snapshot supplied
the value.

The existing descriptor-missing fixture remains a negative fixture. A synthetic
provider-contract test may exercise an explicit zero descriptor, but it must be
labeled synthetic rather than captured. Production never infers zero from
category, fixture name, configuration, or descriptor absence.

This matches Polymarket's current public contract: market information supplies
fee parameters, and the public token endpoint supplies a base fee. The relevant
primary references are:

- <https://docs.polymarket.com/api-reference/markets/get-clob-market-info>
- <https://docs.polymarket.com/api-reference/market-data/get-fee-rate-by-path-parameter>
- <https://docs.polymarket.com/trading/fees>

## Maker leg authority

Introduce a sealed quote-leg authority derived only from `MarketQuote` and its
`MakerOrderLifecycleScopeIdentity`. It contains:

- the lifecycle leg;
- the exact instrument assigned to that leg by the sealed market scope;
- the proposed lifecycle action; and
- the shared market/transaction authority needed to arm and settle the attempt.

Its fields are private. Production construction is restricted to the active
market/proposal path; any test constructor is test-gated and still validates the
scope's leg-to-instrument mapping. `MakerQuoteTransactionContext` also exposes no
public fields from which a caller can assemble a forged pairing.

`MakerQuoteTransactionContext` carries this authority rather than treating the
proposal and command instrument as unrelated inputs. Binding a quote-bearing
command checks one value: the command's typed action and instrument must equal
the sealed authority. The check occurs before order construction, preparation,
registration, admission, or sink invocation.

For submits, the final NT order is checked against the same authority before
provisional registration. Cancel and modify commands are checked before their
NT mutation path. A malformed public command therefore cannot build or mutate a
NO instrument while consuming YES lifecycle capacity, even if both instruments
resolve to the same market scope.

Scope cancel-all remains a separate non-quote capability and cannot consume a
quote-leg authority.

## Quote transaction state design

The current enum combines stable leg state, attempt phase, wind-down mode,
poisoning, and settlement. It then duplicates the attempt and poison variants
inside `WindDownQuoteTransactionState`. Replace that product with shared types.

Conceptually, the top-level state has only:

- a non-attempt state;
- an in-flight attempt; or
- a settlement record.

The non-attempt state contains either an active stable phase, a wind-down stable
phase, or a poisoned hold. Active-only phases such as resting and replacement
backoff do not exist in the wind-down phase type.

The in-flight attempt owns:

- `QuoteTransactionMode::{Active, WindingDown}`;
- one `QuoteTransactionArm`; and
- exactly one phase-shaped budget state:
  `Armed(ArmedQuoteBudget)` or `SinkInvoked(SinkInvokedQuoteBudget)`.

There is one arm/sink reducer. Mode only selects the post-attempt stable result;
it does not select a parallel implementation of arm, sink accounting, unwind,
callback retirement, generation lookup, or registration phase.

The settlement record replaces `route: Option<_>` plus `reopened: bool` with
explicit variants:

- awaiting route settlement after a terminal callback;
- route-settled with stable terminal truth; or
- route-settled and reopened.

It owns a non-attempt stable/poisoned result, never another settlement. Recursive
`Box<QuoteTransactionState>` settlement is removed.

Wind-down is a one-way mode transition. It maps active stable phases once,
changes an in-flight attempt's mode without changing its attempt phase, and
changes a poisoned hold's mode without recreating its payload. Terminal events
retire wind-down states through the same shared attempt/poison operations.

The reducer remains exhaustive, but exhaustiveness is applied to each smaller
sum type. It must not recreate a state-by-event Cartesian table in helper
functions.

## Budget, terminal, and clock types

### Requote liability

Replace `OutstandingLiability { sink_accounting, rest_cost, ... }` with variants:

- one-shot liability;
- cancel/resubmit with both REST halves outstanding; and
- cancel/resubmit with only the replacement half outstanding.

The first sink invocation moves the cancel/resubmit variant to the replacement
outstanding variant. It never determines phase by comparing a numeric cost to a
constant. Costs remain payload values and may become equal without changing
phase behavior.

### Terminal refinement

`MakerQuoteOrderAuthority::terminal_event` first computes a
`MakerQuoteTerminalDisposition`. It then compares that disposition with retained
truth and finally wraps it in `MakerQuoteRetainedTerminal::Terminal`. No branch
can produce `ReopenedFrom`, so no impossible production arm or `unreachable!`
remains.

### Scope-closure time

The actor's `now_ns` observation is threaded through every terminal-settlement
entry point that can trigger retention-scope closure. `ScopeClosure` never uses a
zero or fallback sentinel. Records without economics already carry a real
cancellation intent; closure preserves that deadline or requests one at the
current actor time.

## Forced-reduction authority deletion

Delete `BoltV3FinalOrderEconomicsScenario::ForcedReduction`, its constructor,
and `BoltV3SubmitIntentKind::KillSwitchForcedReduction`. Remove the corresponding
request field, admission evaluator, current producer path, order-execution
branches, and impossible normal-admission arm.

Loaded configuration continues to reject automatic flattening while Slice 1 is
quote-only. Kill-switch planning/proof types may remain only where needed for
that validation or a future non-routing plan. Existing evidence codecs may read
historical forced-reduction facts, but no current evidence producer or recovered
fact can mint a submit permit.

## NT mutation boundary

The cancel coordinator cannot prove from NT's `Result<()>` whether the adapter
sent a venue command. Pinned NT can fail before sending, can return `Ok(())`
without sending when pending-cancel marking declines, or can fail after an
adapter-side effect. Refunding on `Err` would therefore undercount an unknown
dispatch outcome.

The safe boundary remains invocation of the NT mutation method:

- before invocation, abort releases the reservation;
- after invocation, success or error commits the conservative attempt charge;
- later authoritative cache/order observations determine lifecycle progress.

Rename `CommandIssued` and related methods to `NtMutationInvoked` (or an
equally explicit name). Documentation and tests must say that this is an upper
bound on venue REST usage, not proof that a network request left the process.
No cancel `SinkRejected` refund path is added.

## Error handling and invariants

Production code changed in this round follows these rules:

- no `panic!`, `assert!`, or `unreachable!` for a state constructible through a
  public or internal runtime API;
- no wildcard over `ExposureState`, quote transaction phase, maker leg, submit
  intent, or liability phase;
- invalid leg/instrument, fee authority, generation, and recovery payloads fail
  before mutation;
- sink-invoked uncertainty remains fail-closed and is never refunded based only
  on a synchronous NT error;
- historical evidence is inert and cannot construct current runtime authority.

## Verification design

Implementation is differential-test first. Required behavior evidence:

1. A partially filled entry with a still-working remainder enters replacement
   conflict, closes its retained episode, receives canonical `None`, and remains
   `PendingEntry`; a new entry grant is denied until terminal order evidence.
2. The reverse close/projection ordering reaches the identical state through the
   same discharge operation.
3. Missing Polymarket `fd` fails closed under shipped configuration; an explicit
   provider zero descriptor produces provider-sourced `ProvenZero`.
4. Submit, cancel, and modify commands with a mismatched leg/instrument fail
   before build, registration, admission, and sink calls. Correct pairings pass.
5. Every blind-recovery cause has its required payload and retains/release
   behavior; there is no runtime invalid-combination test because the invalid
   constructors do not compile.
6. A stale entry decision records the exact typed reason and routes nothing; no
   panic occurs. The already-armed control is separately classified.
7. Existing maker transaction tests continue to prove pre-sink rollback,
   sink-invoked charging, synchronous callback wins, idempotent settlement,
   conflicting settlement rejection, wind-down retention, and terminal reopen.
8. Cancel invocation errors retain their conservative charge and enter the
   unobserved/backoff path; failures before NT invocation release it.
9. Scope closure uses the supplied actor time and never creates a zero deadline.
10. Forced reduction cannot be constructed as a current economics scenario,
    submit intent, or admission request.

No new source-scanning or structure test is added. Compiler-enforced type
closure plus behavior tests are the evidence.

Run the smallest targeted behavior tests needed during implementation. At the
clean final head, run the lightweight formatting/static checks and diff check,
then push the exact head so advisory CI supplies the remote-first workspace
Clippy, isolated backtesting Clippy, nextest, and build evidence. Report the head
and detach rather than waiting on CI. If GitHub Actions is unavailable, use only
the repository's designated EC2 fallback commands and attach their raw output.

## Conditional census and review evidence

The prior `+1` claim applied only to `5ee3eaf13..524362b68` and only to
`if`-bearing lines. It must not be repeated as a claim about the final repair
range. Two independent external reviews measured
`623801311..524362b68` as `+251 if` and `+222 match`, net `+473` under their
stated production-Rust filters.

Before the next review request, publish:

- the exact range;
- included paths and test exclusions;
- `if` and `match` counts separately;
- per-file totals for the affected production files; and
- the script or command used as a diagnostic attachment, not a repository test.

For `524362b68..new_head`, conditional-bearing production lines in the affected
files must decrease. If they do not, stop and revisit the state decomposition
before requesting review. This diagnostic is a complexity budget, not proof of
correctness.

## PR and tracker updates

After code and local findings are resolved:

1. Add a durable #869 tracking note for the deferred maker
   NT-event-to-lifecycle/event-fence work inherited from #817.
2. Update the stable PR body with the takeover-round exposure authority, maker
   transaction boundary, load-time OMS capability work, structural external
   review repairs, and the #869 remainder. Do not add head SHA or transient CI
   status to the body.
3. Post an exact-head review request containing the corrected census and
   verification commands/results.
4. Request fresh Claude and GPT reviews plus the required native reviewer.
5. Do not merge without explicit user authorization and required native
   approval.

## Rejected alternatives

### Patch each finding locally

Adding a guard to the replacement reducer, an accessor check for the maker, and
more match arms would close individual examples but preserve the duplicated
state products that caused them. This is rejected because it increases the
conditional surface and leaves the compiler unable to enforce shared behavior.

### Defer structural findings

The latent findings are not unrelated cleanup. They are in the transaction and
exposure authority introduced by this PR, and the repository treats every
substantive issue as a finding. Deferring them would leave unresolved review
debt and violate the declared no-debt closure.

### Refund every synchronous cancel error

NT's return value does not establish whether no mutation escaped. Refund on
error can undercount a real or adapter-deferred request. Conservative invocation
accounting with truthful naming is the safe single path.

## Acceptance

This design is complete when its type boundaries, behavior evidence, scope
disclosure, and complexity budget are approved. Implementation planning begins
only after that approval.
