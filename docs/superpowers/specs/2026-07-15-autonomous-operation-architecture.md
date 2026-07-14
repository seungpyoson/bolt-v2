# Continuous Autonomous Operation Architecture

Status: owner-approved design direction; implementation is prohibited until the
finalized package receives `APPROVE` from the independent Claude Code review.

Review base: `origin/main` at
`17bdf952f3e9422c6957b88556dbb4f145046754` (`v0.1.13`). The stopped production
host and its artifacts are incident evidence only. They are not an implementation
base, and this design does not authorize starting the host, deploying, or trading.

## Decision

Adopt one logical fixed-generation **Recovery Capsule** synchronously replicated on
two independently mounted full recovery devices, one fixed evidence-arena replica
beside each Capsule replica, one independently mounted fixed commit witness, and
one lifecycle supervisor.

- The venue owns external facts: accepted orders, fills, positions, balances, and
  settlement results.
- The Recovery Capsule is the sole Bolt-owned authority for unresolved workflow
  state, capacity ownership, and semantic evidence novelty. Its two full copies
  are replicas of one generation graph, never independently writable authorities.
- The 16-KiB witness carries only the selected Capsule digest and parent; it cannot
  reconstruct state or become a competing authority. It votes only through a
  checksum-valid selector naming one fully synced witness record with the exact
  digest, parent, and configured device identity. A missing or corrupt selector
  abstains, and an unselected child is never inferred as a vote. Together the
  three device manifests form a two-vote commit quorum.
- The fixed evidence arena contains immutable historical records waiting for S3.
  It is not a second recovery authority.
- NT caches and in-memory projections are derived and disposable.
- S3 is historical retention only. Startup and risk reduction never read S3.

This is the smallest design found that makes storage cardinality a function of
active risk and configured semantic state instead of event count. A compacted
journal and a fixed-capacity database were considered and rejected below.

## Why the Current Design Cannot Run Unattended

The source audit found structural, not tuning, failures:

- `src/bolt_v3_decision_evidence.rs` appends audit and recovery facts to one JSONL
  file without a writer-side cap. The configured 1 MiB value is only a reader cap.
- Entry-skip and strategy-input dedupe replace the finite novelty mask when a
  volatile outer key changes. Existing tests intentionally allow A→B→A to emit
  again.
- The same JSONL file is both historical audit and recovery truth. A torn final
  write is fatal, and closed orders and settlements accumulate with uptime.
- `src/nt_runtime_capture.rs` explicitly uses an unbounded channel and non-rotating
  local capture. NT engine queues are configured at 100,000 items without byte
  accounting.
- Settlement retry can become terminal locally instead of remaining a bounded,
  automatically retrying unresolved state.
- Rollover uses role-by-role subscription calls and treats transport success as
  subscription success. The pinned NT Polymarket client does expose an
  unsubscribe write, but neither subscribe nor unsubscribe has a server
  acknowledgement. A partial desired-set edit therefore cannot prove the remote
  set. An expired position instrument can re-enter a later desired set, while
  provider tasks and cache entries have no generation-scoped owner.
- Logging, file descriptors, tasks, HTTP bodies, WebSocket frames, retry episodes,
  and cache cardinality do not share an enforceable admission budget.

The observed 1,254,325-byte, 272-record incident is therefore a short witness of
an unbounded design. Making the file larger, rotating it, or extending dedupe
retention would only postpone the same failure.

## Terms

An **EvidenceEpisodeId** is a stable business object, not a sample of current
inputs. For the BTC binary strategy its identity is the hash of only these
non-temporal semantic fields:

```text
strategy logical id
target logical id
venue id
Gamma id
condition id
question id
negative-risk mode
ordered exactly-two (outcome index, normalized outcome, CLOB token id) tuples
```

It never contains a slug, serial/window index, open/close value, price, timestamp,
configuration digest, schema version, deployment id, transient feed flag, retry
number, or diagnostic value. Slugs are excluded because their text may encode a
timestamp. A risk episode adds only a bounded admission ordinal `0..9`, assigned
once within that market. That ordinal is never replenished or reused in the same
market. Entry, replacement-exit, position, and settlement states all remain
canonical states of that one risk episode; order ids are payload, never new
episode identities.

A **canonical state** is a reviewed member of a finite registry, such as
`entry.blocked.missing_fast_book`, `order.submit_unknown`, or
`settlement.observed`. Diagnostic fields are payload only. The stable evidence id
is `(episode_id, canonical_state_id)`. Schema migration preserves this identity and
maps the existing novelty bits; it does not create a new episode.

A separate **GammaMarketBinding** is bound exactly once from the first complete
hydrated exact-slug response. It contains the EvidenceEpisodeId venue fields plus
the exact discovery slug and trusted open/close window. A response without both
CLOB token ids remains the bounded `DiscoveryHydrating` retry state and creates no
episode. Once bound, any byte-normalized binding mutation blocks that lane/serial;
it never rekeys, rolls, or clears an existing EvidenceEpisodeId or novelty mask.
Changing only slug/window/timestamp-bearing discovery metadata therefore cannot
reset evidence suppression. A genuinely new condition/market id may create a new
episode only through the reviewed next-market transition and durable empty barrier.

The TOML registry is closed by these exact id ranges; every id is owned by one
workflow family and unassigned ids are permanently non-emittable:

| Registry | Id range | Family allocation |
|---|---:|---|
| Risk (64) | `0..7` | admission/entry: 8 |
|  | `8..23` | order prepare/submit/fill/terminal: 16 |
|  | `24..31` | position/exposure: 8 |
|  | `32..47` | exit/cancel/replacement: 16 |
|  | `48..55` | settlement/redemption: 8 |
|  | `56..61` | reconciliation/dependency: 6 |
|  | `62..63` | terminal/integrity: 2 |
| Market (256) | `0..31` | discovery/identity: 32 |
|  | `32..79` | lifecycle/rollover: 48 |
|  | `80..143` | subscription/book: 64 |
|  | `144..207` | strategy-input/pricing-blocker: 64 |
|  | `208..239` | dependency/health: 32 |
|  | `240..255` | terminal/closed-window-skip: 16 |
| System (64) | `0..15` | startup/recovery: 16 |
|  | `16..31` | storage/archive: 16 |
|  | `32..47` | authentication/network/provider: 16 |
|  | `48..59` | capacity/host: 12 |
|  | `60..63` | integrity/operator: 4 |

The #1354 source census must assign every existing producer to a named TOML member
inside its family and reject duplicate numeric ids, unknown names, family overflow,
or an unassigned emission at startup/compile-time generation. Every later workflow
PR updates the same registry and proves all of its transitions map within the frozen
family allocation; it cannot create a fourth registry or dynamic string state.

An **active risk** is any entry intent whose fixed request and reservations are
durable as `EntryPreparedNotAuthorized` until its order, resulting position, exit,
and settlement are all authoritatively terminal. Timeouts and missing events never
make it inactive.

### Frozen provider evidence boundary

The design was checked against this exact source set. These revisions are review
evidence, not permission to copy deployed artifacts or to assume hosted behavior.
`AO-NT.b` and `AO-REDEEM` must pin the implemented revisions, regenerate the
boundary manifest, and fail the autonomous profile if any source or conformance
digest differs.

| Boundary | Reviewed revision | Contract used by this design |
|---|---|---|
| bolt-v2 | `17bdf952f3e9422c6957b88556dbb4f145046754` | Current implementation and incident base only |
| nautilus_trader fork | `afc014a55b51463641cc19c68bffe25cdac6588a` | Current NT Polymarket, lifecycle, task, queue, and transport boundary |
| Polymarket ctf-exchange-v2 | `ccc0596074f4dfd62c944fbca4de252893b82b4b` | V2 order structure, validation, fills, pause, and operator preapproval semantics |
| Polymarket clob-client-v2 | `ff5913f83132a141e01d403e505b6ccc003aa0f7` | POST/cancel/status client behavior and request construction |
| Polymarket rs-clob-client-v2 | `3ae1aae5e9ded38f984464c9fc0f307f8a9f41fb` | Rust order construction and FOK boundary |
| Polymarket py-clob-client-v2 | `fdb2590dc85e600ad98f1f668ea62a0627554d73` | Independent route/schema comparison fixture |
| Polymarket builder-relayer-client | `9122f6fb1856f1ecfe4406685bfa19a2c5a7b290` | Explicit Safe nonce request construction and relayer states |
| Polymarket py-builder-relayer | `267a36d84d7839b6e4ac134297d9230fc224cf8f` | Independent relayer route/body comparison fixture |

Route contracts are decoded separately under the common 2,097,152-byte HTTP-body
cap. Wire strings are not shared internal enums:

| Route | Exact accepted success schema |
|---|---|
| POST insertion | Exact fields `success=true`, absent/empty `errorMsg`, `orderID` 1..256 bytes, `makingAmount` and `takingAmount` exact-decimal strings 1..128 bytes, wire field `transactionsHashes` with 0..64 canonical 32-byte values, `tradeIDs` with 0..64 values each 1..256 bytes, and lower-case status `live`, `matched`, `delayed`, or `unmatched` |
| GET exact order | Route-specific object with status exactly `ORDER_STATUS_LIVE`, `ORDER_STATUS_INVALID`, `ORDER_STATUS_CANCELED_MARKET_RESOLVED`, `ORDER_STATUS_CANCELED`, or `ORDER_STATUS_MATCHED`; the exact-id object/item and every string/decimal field obey the generated provider manifest |
| Exact-order associated trades | At most 64 items; each status is exactly bare wire value `MATCHED`, `MINED`, `CONFIRMED`, `RETRYING`, or `FAILED`, with every id/string/decimal bound in the generated provider manifest |

A transport failure, non-2xx response, `success=false`, or malformed success
envelope becomes one bounded internal `PostDiagnosticFailure` containing only the
transport class, optional HTTP status, body length, and SHA-256. It is never a wire
status or no-effect proof, and raw error text is not persisted. Signed order/Safe
request bodies, signatures, authorization headers, every SSM credential value, and
all raw provider success/error/request bytes are statically barred from every
log/evidence/alert formatter; only fixed redacted ids, lengths, classes, and digests
may cross that boundary. Extra fields,
cross-route aliases, item 65, oversize fields, or unknown values fail closed. The
current POST API schema omits `unmatched` while the official lifecycle contract
includes it, so captured route-specific fixtures must settle that drift before
enablement.

Source inspection establishes three negative capability facts. Current public V2
does not expose the permanent order-hash tombstone required after an ambiguous
entry dispatch. Its POST response exposes a dynamically sized transaction-hash
vector but no reviewed completeness or at-most-64 guarantee. The reviewed relayer
body accepts an explicit Safe nonce, but no reviewed contract guarantees that the
hosted relayer will accept and order a competing same-nonce fence. Each fact has
its own negative activation fixture; a provider documentation claim alone cannot
turn a gate green. Hosted
conformance must prove the exact contract against a non-trading test identity, and
any later source or behavior drift disables the profile before risk increases.

## Selected Architecture: Fixed-Generation Recovery Capsule

### Authoritative state and data flow

Each of two independently mounted full recovery devices contains two preallocated
1-MiB Capsule slots, two 4-KiB manifest allocations (selected manifest and
temporary), and one fixed arena. A third independently
mounted device contains two 4-KiB witness slots, one 4-KiB selector, and one
4-KiB selector temporary. Each record names an exact digest and parent; neither
numerical generation nor device priority selects authority. A logical state is
committed only when the same synced digest has votes from two distinct configured
device identities, at least one of which is a full replica. All fields have fixed
maxima and a bounded binary encoding.

A witness vote exists only when the selector checksum is valid and the selected
witness slot is fully synced and matches the selector's digest, parent, and
configured witness device identity. A missing or corrupt selector makes W abstain;
the loader never promotes an unselected valid-looking child. Thus A+B may select
authority and repair W, while one full replica plus an invalid W has one vote and
no quorum.

Each slot contains:

- ten active-risk records;
- twenty unresolved-order records;
- ten settlement records;
- thirteen active episode records and their novelty bitsets;
- fourteen Polymarket lifecycle-bundle records;
- the fixed evidence receipt/ready bitsets, one full prepared evidence record, and
  fixed compact pending-evidence receipts inside every episode/risk slot;
- one prepared S3 object descriptor (never its multi-megabyte payload), 365 fixed
  ring descriptors and one legacy-slot state, capped retry states, health latches, configuration
  fingerprint, migration state, and the one-lane contiguous window-retirement
  frontier.

Raw client order, venue order, fill, trade, and settlement identifiers are allowed
only inside these bounded workflow sections because venue reconciliation requires
them. They are payload, never episode identity. Their evidence projection is HMAC-
pseudonymized or omitted before a receipt/arena frame is built; raw identifiers
never enter the immutable arena, S3, or ordinary logs.

The healthy write protocol is:

1. Build and size-check the complete successor in bounded memory.
2. Overwrite the inactive slot and publish the exact manifest on replica A.
3. Publish the same synced child on replica B, creating a full-replica quorum.
4. Sync the same digest/parent into a witness record, then sync its checksum-valid
   selector. Require all three selected votes before reporting a risk-increasing
   transition committed or authorizing its effect.

For a transition that cannot increase risk, any two-vote quorum may commit: A+B if
the witness is unavailable, A+W if B is unavailable, or B+W if A is unavailable.
The writer never sends an external effect before that quorum is synced. The device
records are single-vote registers, so two different current digests cannot each
hold a quorum; every two-vote set intersects and one device cannot select two
digests simultaneously.

If exactly one recovery voter becomes unavailable, entry admission closes before
another provider effect. The remaining two-vote quorum may continue only workflow,
reconciliation, exit, settlement, evidence-receipt, and terminal transitions that
cannot increase risk, using the same durable-before-effect ordering. It records
`ReplicaDegraded`; it never creates a second logical lineage. When the voter
returns, one fixed repair owner validates device identity, selects the existing
quorum digest, directly copies the current full state/arena when needed, and
requires all three voters plus both arenas to agree before entry reopens. A stale
lone replica never has a quorum and cannot act. Repair is automatic and keeps no
lineage history or backlog, even after arbitrarily many degraded transitions.
An A+B quorum may rewrite and select W, but one full replica cannot use a missing
or corrupt witness selector as a second vote.

Each payload is written directly into its inactive preallocated slot; there is no
payload temporary. With `P=1,048,576`, `M=4,096`, arena `A=31,457,280`, and
witness `W=16,384`, one full replica's crash peak is
`2P+2M+A=33,562,624` bytes. Two full replicas plus the witness are exactly
`67,141,632` bytes. Retained recovery-migration inputs add 4,325,376 bytes, so
cutover is `71,467,008` bytes below the configured 157,351,936-byte aggregate
recovery ceiling. Each device has its own project quota and free-floor claim;
neither borrows another's capacity.

Both the state before and the state after every manifest publication are valid
restart states. A fully synced but unpublished inactive slot is never silently
chosen as current. Every logical slot must pass through a durable empty/reusable
barrier before its identity changes. Consequently two adjacent Capsule versions
can contain the same identity in two workflow phases, an identity versus empty, or
empty versus a new identity—but never two different identities that require an
extra slot. The same barrier applies to prepared evidence, prepared archive work,
retry owners, and outbox-slot reuse.

Within a full replica, if the manifest is lost, its two slots must prove a direct
parent-digest relation. Across voters, a same-digest quorum is authoritative and
may repair a stale voter across any lineage distance. Without a quorum, one
selected child and its direct parent are the only repairable divergence.
A field-by-field fixed-layout join chooses the more conservative workflow phase,
ORs novelty and ownership bits, retains the more restrictive kill-switch/health
state, and then venue-reconciles. The join is mechanically proven closed under the
same 10/20/10/13 maxima. If adjacency or any field join is not provable, Bolt does
not union into overflow storage; it enters integrity halt and reconstructs
venue-observable exposure while retaining conservative local policy.

The join is not a generic merge. The schema generates this closed repair algebra:

| Field family | Allowed adjacent join | Repair behavior |
|---|---|---|
| Header/schema/config | Exact equality, with one slot naming the other's digest as parent | Any mismatch is integrity halt |
| Empty/identity | Same identity in adjacent phases, identity versus durable empty, or empty versus new identity | Carry the identity and all its capacity as `QueryOnly`; the empty barrier proves no second identity is needed |
| Capacity/ownership | Bitwise OR for the same logical owner | Release nothing until authoritative reconciliation |
| Entry preparation before send | A child-only `EntryPreparedNotAuthorized` remains that exact source-fenced phase with its fixed request and reservations | It has provably never had send authority; repair may only repeat the final predicates into a fresh all-three `DispatchMayHaveStarted` commit or durably terminate as all-three `EntryAborted` |
| Dispatched or incomparable risk-increasing workflow | `DispatchMayHaveStarted`, or any incomparable phase where a provider call may have started, remains the same conservative state in the existing slot | Never send/replay during repair and never infer absence. Require the exact provider terminal certificate; otherwise retain query-only capacity and block entry |
| Exit/order/settlement workflow | Generated monotonic phase lattice; incomparable phases become `NeedsAuthoritativeQuery` | Retain request digest and maximum exposure/capacity; query before replay, terminalization, or release |
| Request/operational identifiers | Exact byte/digest equality whenever both slots contain them | Mismatch is integrity halt; raw ids never enter evidence identity |
| Novelty, risk ordinal mask, receipt state | OR novelty/masks; receipt lattice preserves the earliest not-yet-proven materialization and any exact prepared digest | Never clear novelty or reuse an ordinal; digest/coordinate conflict is integrity halt |
| Archive descriptor/ack | Same deterministic object: retain local ownership unless both prove the same acknowledgement | `HEAD`/checksum before free; different object identities require the prior empty barrier or halt |
| Kill switch/health | Most restrictive value; readiness predicates combine with logical AND | A possible halt cannot be cleared by venue facts |
| Retirement/lifecycle | More advanced contiguous frontier plus OR masks, but desired WS leases are recomputed from trusted expiry before any wire action | Frontier never releases risk; ambiguous/expired lifecycle remains REST/query-only |
| Retry/timer | OR owner, earliest due time, greatest saturated backoff stage | One existing retry slot; no attempt history |
| Diagnostic counters | Ignored for authority; fixed-width wrapping value selected deterministically | Cannot affect any safety decision |

Every workflow enum carries a generated pairwise join table. The exhaustive product
test constructs every legal adjacent pair at maximum cardinality and proves the
result encodes in one Capsule. A synced but unpublished child can suppress an
effect; only a may-have-started dispatch phase can trigger a provider query. It can
never authorize a new risk-increasing effect.

Orders use a two-phase provider interface. NT prepares the exact signed request and
its deterministic expected venue order id without sending it. Bolt atomically
stores that fixed request, its candidate snapshot, and every lifecycle reservation
as durable `EntryPreparedNotAuthorized`; that state is never send authority. If the
candidate is stale, all three voters commit `EntryAborted` before any send-capable
state exists and release only after that terminal commit.

Otherwise one final all-three commit rechecks trusted time, market/current-expiry
status, required feeds, complete entry health, and the retained capacity vector and
stores the finalized pre-dispatch Polygon block, exact request/hash, final predicate
digest, and `DispatchMayHaveStarted` together. That conservative state is durable
*before* the first provider syscall. There is no separately durable
`EntryAuthorized` or post-write "started" marker: a crash before the syscall is
intentionally repaired as an ambiguous dispatch. Only the live owner that made the
final commit may issue that one syscall. Every restart from
`DispatchMayHaveStarted` queries or permanently fences the exact hash and never
aborts, replaces, or resends it merely because a provider read is negative.

If A+B contain `EntryPreparedNotAuthorized` before W is repaired, selecting W may
only reproduce that non-authorizing state. Repair never promotes or sends it; the
candidate must pass a fresh all-three final commit or become `EntryAborted`. If any
voter contains `DispatchMayHaveStarted`, repair keeps the full reservation and
enters the exact-hash recovery path.

That recovery path requires a source-fenced `ProviderTerminalCertificate` with one
of two meanings:

- `Filled`: the exact order hash, final matched quantity, and a complete sorted
  unique set of at most 64 canonical 32-byte transaction hashes are immutable.
  Bolt verifies every finalized receipt and indexed V2 `OrderFilled` log plus exact
  account post-state before booking the fill.
- `PermanentlyTombstonedNoEffect`: one linearizable provider operation has ordered
  the exact hash behind every submit, delayed-execution, retry, match, duplicate,
  and preapproval queue and has durably made all future execution impossible. Bolt
  additionally requires untouched finalized V2 status and exact account post-state
  before releasing the reservation.

The provider contract must accept a tombstone for an absent or not-yet-indexed
hash, distinguish that state from "not found" and "already canceled", survive
provider restart/rollback indefinitely, reject every duplicate submission after
the tombstone, cover operator-preapproved orders, guarantee and return the complete
at-most-64 transaction-hash set for a fill, cap each sequential receipt at
2,097,152 bytes and 4,096 log items, and publish those exact response/item/byte
maxima. A
CLOB `404`, `not_canceled`, ordinary cancel acknowledgement, order status, trade
status, heartbeat cancellation, elapsed time, or quiet chain is not this
certificate. Associated trades and WebSocket messages are bounded diagnostics,
not recovery authority.

The current public V2 boundary does **not** satisfy that contract. Its signed order
has a creation timestamp but no signed expiry, nonce, or maker-controlled on-chain
cancellation; only the operator can invalidate a preapproval. The CLOB wire
`expiration` is unsigned, cancellation conflates absent and already-canceled
orders, and no public contract makes a cancellation an indefinite linearizable
tombstone against delayed/retry work. Consequently the autonomous profile must
remain mechanically incapable of risk-increasing dispatch until a pinned provider
revision supplies and passes this contract (or a separately reviewed custody and
signature architecture supplies equivalent user-controlled permanent
invalidation). `AO-INTEGRATION` may not enable autonomous entry while this
precondition is false.

The prepared record also stores the exact exchange address and a finalized
pre-dispatch Polygon `(block_number, block_hash)`. At recovery, Bolt calls V2
`getOrderStatus(order_hash)` at a configured finalized block. Its closed truth
table is:

| `filled` | `remaining` | Proven meaning |
|---|---:|---|
| false | 0 | Untouched at that block only; it does not prove that the off-chain FOK can never execute later |
| false | `0 < r <= makerAmount` | Partial on-chain maker fill of exactly `makerAmount-r`; this violates the expected FOK outcome and remains integrity-halted |
| true | 0 | Full on-chain maker fill of exactly `makerAmount` |
| true | nonzero, or `remaining > makerAmount` | Contract/schema conflict; halt |

Status storage proves maker quantity only. It does not prove executed taker
quantity/cost, fee, or no-future-fill. For a mined trade, the exact transaction
receipt is fetched by transaction hash and must contain compatible logs from the
prepared exchange address. V2 `OrderFilled` has indexed `orderHash`; its exact
`makerAmountFilled`, `takerAmountFilled`, and `fee` are the accounting truth. Logs
are aggregated by `(transaction_hash, log_index)` and their maker total must equal
the finalized status transition. A mismatch, duplicate coordinate with different
bytes, reorg, or wrong exchange blocks terminalization.

If the certificate claims a fill but omits, supplies item 65, duplicates, or
conflicts on its complete transaction-hash set—or any receipt exceeds 2,097,152
bytes/4,096 logs—Bolt remains `Unknown` and entry stays blocked. The provider
capability manifest must guarantee those maxima for every permitted order; local
truncation is not liveness. Bolt performs at most 64 sequential exact-hash receipt
queries per order and does not scan an uptime-sized block interval or account
history. The certificate is therefore both the semantic terminal proof and the
closed-form work bound. No finite elapsed-time or FOK-inclusion-horizon assumption
receives safety credit.

A negative/absent CLOB result or an untouched status never authorizes replay or
release. After `DispatchMayHaveStarted` there is no risk-increasing replay; only the
exact terminal-certificate protocol may resolve the slot. Until it does, the full
reservation remains and new entry is blocked as required by capacity. A temporary
finalized zero pUSD allowance may fence BUY settlement while preserving SELL exits,
but restoring that allowance would revive the old V2 signature and is forbidden
without the permanent certificate. Thus allowance revocation is a bounded
protective step, not a substitute for the missing provider primitive.

Risk-reducing and settlement actions use the same prepared/unknown protocol; there
is no undurable provider bypass. Logical capacity, archive, logging, S3, ordinary-
memory failure, and failure of any one recovery voter cannot reject them: the
remaining full-replica quorum durably prepares the exact action before send. New
risk remains blocked until automatic repair makes all three voters and both arenas
identical.

If fewer than two recovery voters, or no current full replica, are available, Bolt
initiates no new external effect. It retains bounded in-memory/venue risk knowledge,
probes all three voters in fixed retry state, and automatically resumes durable
reconciliation/reduction/settlement when a current full replica and matching second
vote return. Destruction of both full media may require rare operator replacement,
but never guessing, S3 recovery, or new risk. The design deliberately rejects the
unavailable “one stable order id closes until flat” venue primitive.

The autonomous profile requires an exclusive Bolt trading account/wallet: no other
writer may create orders, positions, transfers, or redemptions. Current unresolved
capture requests one more than each local maximum (`21` open orders, `11` active or
redeemable positions/claims) and accepts only a complete response with no
continuation. The extra item, unknown writer, or incomplete cursor halts entry and
repair. Provider models decode sizes, prices, balances, claims, and fees directly
to validated exact decimal/scaled-integer types; an `f64` field, lossy
number-to-string round trip, unsupported scale, or range overflow is a boundary
error. Capture includes every positive position and redeemable claim, including
amounts below the venue execution-size threshold. Dust remains reserved,
reportable, and redeemable even when it cannot be sold. Known prepared identities
use only the bounded exact-order/trade/receipt/status workflow above; Bolt never
scans account-wide terminal history.

### Semantic evidence and the fixed arena

Each recovery device has one preallocated 31,457,280-byte arena replica divided
into 960 fixed 32,768-byte slots. The pair represents one logical arena and cannot
grow, create forever-unique filenames, or retain directory high-water allocation.
Every slot contains a bounded envelope, stable evidence id, actual payload length,
key version, and checksum; unused bytes are padding.

Every canonical state has a fixed receipt index and a fixed arena offset within its
episode partition; there is no general allocator. Production and materialization
use one path:

1. A raw producer calls the typed registry. If that receipt is not `Unseen`, return
   success without changing it.
2. Encode the bounded canonical receipt and atomically commit
   `Unseen -> PendingArena` with its novelty bit and any workflow transition. This
   commit is the one logical evidence-production event.
3. The single materializer scans fixed receipt order. It derives a frame only from
   the receipt and immutable episode fields, then atomically commits
   `PendingArena -> Preparing` plus the global `EvidencePrepared` bytes, receipt
   coordinates, fixed arena offset, and digest.
4. Write those exact bytes at the fixed offset. `Ready` requires both arena
   replicas synced; with one full device unavailable, write the survivor but retain
   `Preparing` and the exact global frame while `ReplicaDegraded` blocks entry.
5. After repair makes both offsets identical, atomically commit
   `Preparing -> Ready(slot,digest)` and clear the global frame.
6. After the deterministic terminal object is remotely acknowledged, commit
   `Ready -> Archived`. Novelty and `Archived` remain until episode retirement and
   the durable empty/reuse barrier.

A crash before step 2 produced no canonical evidence. A crash after step 2 resumes
from the receipt, not raw input. A crash after step 3 rewrites the global exact
bytes. The ordinary producer's novelty no-op never suppresses materialization,
because only the dedicated `PendingArena -> Preparing` transition may advance an
existing receipt. This is the evidence-specific invariant under A→B→A forever.

Each of the ten risk slots contains 64 fixed 128-byte semantic receipts, each of the
current/next market episode slots contains 256 fixed 16-byte receipts, and the
current system slot contains 64 fixed 64-byte receipts. A compact receipt records
canonical state, disposition, bounded core facts or their fixed digest, and whether
optional diagnostics were unavailable; the materialized minimal record never needs
the raw input. Arena failure therefore leaves every receipt `PendingArena` while all
ten risks close and settle. When storage returns, fixed-order materialization
continues automatically. Permanent media failure retains the receipts and blocks
new risk but not venue reduction.

The arena partitions are disjoint:

- 640 slots: 64 reserved for each of ten active risks;
- 256 slots: one materialized market episode;
- 64 slots: the current market's system/recovery state.

There is no general runtime evidence partition. Autonomous evidence must belong to
one of those finite episode registries and therefore to one of the twelve terminal
objects. One-time legacy history uses the separately bounded source-stream uploader;
it never consumes arena slots or creates an additional steady producer.
Every imported `(episode, canonical state)` is owned only by that legacy stream.
Migration seeds current Capsule novelty as `ImportedLegacyOwned` and places older
episodes behind the reconciled retirement frontier, so recurrence cannot create a
second arena copy while upload is pending or after acknowledgement. The arena starts
empty and contains only post-cutover states not present in the imported stream.

Ordinary history cannot borrow closure reserves. A candidate risk receives an
entire empty 64-slot partition before any order can be sent. Its risk ordinal and
episode slot remain occupied until both the risk and its market are terminal, even
if S3 is healthy; terminal risk slots cannot be recycled into an eleventh risk in
the same market. The finite evidence registry must mechanically fit its partition;
unknown or oversized states are a startup error, not a spill file.

Current and next lifecycle metadata and pending receipts fit in their Capsule
episode slots, but only one market may own the 256 materialized arena slots. The
next market can be discovered and subscribed while the current owns them; it cannot
materialize market history or admit risk until the current terminal object is
acknowledged, the current system object is acknowledged, both partitions pass
through their durable empty barriers, and ownership transfers. An S3 outage may
therefore pause rollover entry, but it cannot consume a risk closure partition.

### S3 archive and retention

One Rust-native worker creates deterministic terminal-episode bundles: at most one
256-record market object, one 64-record system object, and one 64-record object for
each of the ten non-replenishing risk ordinals. Thus a market can create at most
twelve objects and 960 records regardless of raw event or health oscillation. Keys
come only from the finite namespace
`history/ring/{000..364}/{0000..3455}`; each day has 288 fixed market positions and
each market has fixed market/system/risk-0..9 object positions. Day serial, episode,
and digest live in the envelope, never in a growing key name. Conditional PUT
therefore occupies an existing fixed position and adds no extra in-flight key.

The Capsule commits the exact arena slots and bundle digest before upload.
Identifiers are classified and pseudonymized before arena creation, not rewritten
during upload. SSM is the only source for the HMAC key; AWS access uses the
configured Rust SDK/instance role. Stored pseudonyms and non-secret key version are
sufficient to upload already prepared records after key rotation.

PUT is conditional. A lost response is resolved with `HEAD` and checksum/length
verification. If the pending key exists with wrong content, the worker retains the
local object, verifies exclusive IAM/bucket state, deletes that exact corrupt key,
lists/HEADs it absent, and conditionally recreates it; it never overwrites unknown
bytes in place or frees local state first. If exclusivity cannot be proven, PUTs
stop. The Capsule records the verified acknowledgement before any slot becomes
reusable. Retry uses one mutable state, one timer, a saturated backoff index, and no
attempt history. Corruption discovered only after local acknowledgement is a
bounded historical-loss alert; it cannot alter recovery, grow local state, or by
itself block risk management/admission.

Retention uses a fixed ring of 365 daily market-cohort slots plus one fixed legacy-
cutover slot in a dedicated bucket that has never had versioning enabled. A market
slot is `Unverified -> EmptyVerified -> Owned -> ReuseBlocked -> DeletePrepared ->
Deleting -> VerifyingEmpty -> EmptyVerified -> Owned`; no key in that slot's prefix may be PUT for a new day until
all local references/prepared PUTs are zero and delete plus complete list has
durably established `EmptyVerified`. A late unresolved risk pins the slot and blocks
new archive-day admission, not closure. Trusted UTC can rebase the fixed ring cursor
without walking downtime; reuse after any number of day-serial/ring wraps is
authorized solely by the local-reference and remote-empty barriers, never elapsed
ordering.

Before the first owner is admitted, one fixed cursor performs exactly 366 bounded
`ListObjectsV2(prefix, MaxKeys=1)` checks: all 365 market prefixes and the legacy
prefix. `Unverified -> EmptyVerified` commits only for an empty result. Any
pre-existing unexpected key integrity-halts before PUT because it would invalidate
the object/byte bound; there is no discovery list or adoption path.

The legacy slot uses only `legacy/{000..257}` and is never reused for another
migration. It is not eligible for pruning while any object is unacknowledged. Once
all at-most-258 objects are acknowledged, the worker revalidates every object
through one bounded cursor under its exclusive prefix lock; only then may the
migrator commit `DeletionAuthorized`. Its 365-day retention starts at that commit,
not at the first PUT. At expiry it uses the same delete/list/empty barrier and then
commits registered `S3-LEGACY-001: EmptyVerified -> Retired`; the legacy namespace
is never reused. Thus the
legacy objects remain inside the global maximum even if upload takes arbitrarily
long, but they cannot be pruned before they safely replace the local source.

Startup requires `GetBucketVersioning` to return no status; `Enabled` or
`Suspended` is invalid because old versions/delete markers would defeat the proof.
IAM grants exclusive write/delete authority to the configured prefix. S3 lifecycle
is defense in depth and receives no credit in the bound. If listing, deletion,
bucket state, or checksum verification is unavailable, PUTs stop. The local arena
then fills to its fixed cap and blocks new risk while existing-risk partitions
remain usable. When S3 returns, the same worker drains and admission reopens
automatically.

Retention never persists an opaque SDK continuation token. A market cleanup owns
one numeric key index `0..3456`; a legacy cleanup owns `0..258`. It issues fixed
key-name `DeleteObjects` batches of at most 64 with a 262,144-byte response cap,
durably advances the numeric
index only after an acknowledged batch, then HEAD-verifies every expected key by
the same numeric index. One final `ListObjectsV2(prefix, MaxKeys=1)` with a
262,144-byte response cap proves that no unexpected key remains. A crash can only repeat
one idempotent batch or HEAD. An unexpected key is an integrity halt under the
exclusive prefix contract, not a cursor, spill file, or unbounded repair scan.

### Capacity admission

Capacity is a componentwise vector, not a percentage or a disk-only check. A new
risk must atomically acquire all of these before provider preparation or submit:

```text
Capsule risk/order/settlement/episode slots
64-slot risk evidence partition
market and archive headroom
memory bytes and queue bytes
async-task and OS-task tokens
file-descriptor and connection tokens
instrument, cache, subscription, HTTP, and retry tokens
filesystem floor and archive-retention health
```

Failure of any component rejects only the candidate. The detailed initial values
and formulas are frozen in the invariant contract. The candidate cannot cross a
boundary between a check and preparation because the candidate snapshot, fixed
request, acquisition, and `EntryPreparedNotAuthorized` are one serialized Capsule
transition. It also cannot cross from preparation to send: one final all-three
`DispatchMayHaveStarted` commit repeats the trusted-time, market/expiry, feed,
health, and retained-capacity checks against that same snapshot and atomically
stores the finalized block and final predicate digest before the syscall. Failure
durably aborts without sending before capacity is released; a crash after the
final commit is intentionally treated as ambiguous.

### Recovery and corrupt state

Normal restart reads the two full-replica manifests and only the witness record
named by a checksum-valid selector. It selects a same-digest two-vote quorum
containing a full replica, validates that replica and the logical arena, recreates
bounded supervisors, and reconciles every unresolved item with the venue before
admission. A missing/corrupt selector makes W abstain, even if an unselected slot
looks like a child. Entry additionally requires all three selected voters and both
arenas to be identical. It does not enumerate S3 or read legacy JSONL.

If one voter is corrupt or unavailable, the other two matching votes drive
reduction while entry remains `ReplicaDegraded`; repair is automatic. A stale full
replica may be replaced directly from the quorum-selected full copy regardless of
lineage distance, and an A+B quorum may rebuild W's record and selector. One full
replica plus an invalid witness selector is not a quorum. If no quorum exists and
the best valid full copies are only direct parent/child, the closed join applies
under an integrity halt. Loss of the only payload for a quorum-selected digest
sets `HaltedUnknownIntegrity`. A stale full copy is not eligible for catastrophic
bootstrap and venue aggregates cannot replace it. The system waits for exact
payload recovery or for authenticated decommissioning of both original full-media
device identities. Only after neither original full replica has any valid current
payload, no closed join is possible, and a persistent device-epoch fence makes
every old lineage permanently ineligible to vote may the catastrophic-bootstrap
protocol below create a conservative quarantine state. That state may
reconcile/report and perform only a proven aggregate-reducing action or exact
redemption; it cannot submit an entry because the provider exposes no causal proof
that a lost prior action is absent.
Venue facts cannot reconstruct or clear lost non-venue policy, risk ordinals, or
booking associations. The quarantine commits a durable
`UnknownIntegrityEvidenceFence`: it emits no canonical evidence because novelty
truth might already have emitted a now-unobservable state.

Destruction or authenticated permanent decommissioning of both original full
media, including all four full-replica slots, is a catastrophic event rather than
normal automatic recovery. A stale, returning, or merely unreachable full device
does not satisfy this condition. Before writing replica A, the replacement-device
epoch and persistent old-lineage fence must be selected by the signed immutable
device-admission manifest of the active release. That fixed 4-KiB record is inside
the already-budgeted release image/inode, not a fourth recovery ledger; its digest
is copied into the certificate. A fixed current/staging selector pair consumes
8,192 bytes and two inodes inside the already-budgeted system-mutable project; it
is the sole deployment/mode authority, never recovery truth. Each fixed 4-KiB
selector record contains schema, release digest, immutable admission-record
digest, device epoch, one closed runtime mode (`Legacy`, `Migration`,
`CapsuleDisabled`, or `Autonomous`), provider/resource/review manifest digest,
predecessor-selector digest, one fixed action-specific authorization envelope, and
checksum.

Every operator authorization is a fixed action-specific envelope embedded in the
target record. Its signed message is
`domain || transition_id || expected_current_record_sha256 ||
expected_inactive_before_sha256 ||
expected_current_and_inactive_(st_dev,st_ino) || target_core_sha256 ||
prerequisite_evidence_root`. Computation is non-circular: under the lock, Bolt first
captures the selected current and not-yet-mutated inactive records plus both
inode/device identities; builds the full 4,096-byte target with the authorization-
envelope and checksum regions zero; hashes those
bytes as `target_core_sha256`; and computes the prerequisite root. It then releases
the lock without modifying either selector and emits the exact 256-byte challenge
from the 4,096-byte Ordinary union workspace to an operator-controlled signer
outside Bolt, SSM, and the host. The
operator private key is never present in runtime; Bolt has only the immutable
verification key. Current legacy or `CapsuleDisabled` risk management continues
while one fixed in-memory challenge/response slot holds the challenge and its
deadline. The operator invocation owns the deadline; there is no background retry
owner, task, timer, queue, or persistent approval state. Timeout, rejection,
process crash, or stale response clears/recomputes that one slot and changes no authority.

That slot is one 4,096-byte Ordinary buffer phase-reused for target construction,
challenge, response, and final reconstruction; no second target buffer exists. The
signed challenge is exactly 256 bytes: 16-byte schema/transition/flags header,
8-byte expiry, 8 zero-reserved bytes, three 32-byte hashes (current,
inactive-before, target core), one 32-byte prerequisite root, four 8-byte
current/inactive device+inode values, one 32-byte Ed25519 verification-key id, and
32 zero-reserved bytes. The response is exactly 512 bytes: that challenge, one
64-byte Ed25519 signature, and 192 zero-reserved bytes. Unused bytes in the 4,096-
byte union are zero. After verification, the 512-byte response is copied to a fixed
region of the selector-mutator's already-reserved 1-MiB thread stack; that exact
512-byte response is also the selector record's authorization-envelope region. The
Ordinary buffer is zeroed and reused to deterministically rebuild the complete 4,096-byte
target, whose core hash must equal the signed value, before inserting the envelope
and checksum. Thus peak heap ownership remains exactly 4,096 bytes, while the stack
copy is already included in the full thread-stack reservation.

When a signature returns, Bolt reacquires the lock, recaptures both pre-records and
identities, rejects any challenge mismatch, verifies the operator signature, and
only then inserts the complete envelope. It computes/inserts the record checksum over the record with
only the checksum region zero. Verification reverses those steps and requires every
embedded field to equal the recomputed value before signature acceptance.
`target_core_sha256` therefore covers all target bytes outside the envelope and
checksum. The complete prepared-target record SHA-256 is computed only after the
envelope and checksum exist; it is an exchange/reopen guard, not a signature input,
which avoids a signature-hashes-itself cycle.
`prerequisite_evidence_root` covers the generated ordered 64-slot prerequisite
digest vector with absent slots zeroed. The record checksum then covers the whole
record including the envelope. An authorization is therefore invalid for another
edge, predecessor, inode mapping, target release/device/mode, or evidence set.

One selector-mutator owner serializes every initialization, release, device, and
mode edge. It takes a nonblocking exclusive lock on the already-budgeted selector
parent directory file descriptor, so no lock inode is added, and holds it
continuously from before the first inactive-record mutation through record sync,
exchange, parent sync, and reopen verification or abort. Before that first write,
the mutator rereads and compares the expected current digest, expected inactive-
before digest, and both fixed `(st_dev,st_ino)` identities. After writing/syncing
the target and immediately before `RENAME_EXCHANGE`, it rechecks that the current
digest and both identities are unchanged and that the inactive inode equals the
complete checksum/signature-valid prepared target with the recomputed signed core.
Any mismatch aborts without publication. The full prepared-target SHA-256 then
defines the exact post-exchange/reopen mapping. Source fences make the mutator the
only writer of either inode. Crash releases the kernel lock, and restart reacquires
it and accepts only the exact pre- or post-exchange inode/digest mapping.

`SELECTOR-INIT-001..004`, owned by `AO-HOST`, are the only adoption path from the
current direct-launch legacy install. After explicit operator initiation they stop,
mask, and drain the legacy unit; bind both fixed records to the exact immutable
legacy release and `Legacy` mode with no autonomous authorization; prove the pair
shares one filesystem and exercise supported `RENAME_EXCHANGE` on the still
non-authoritative pair; sync/reopen the records and parent; and durably replace the
masked direct launcher with the immutable selector-only launcher before unmasking
it. Before the launcher replacement, the stopped direct launcher is the sole
authority. After it, the selector names the same legacy release as sole authority.
The host transition supervisor is independently restart-enabled, so a crash while
the unit is masked resumes the same bounded step without an operator; no state can
run both launch paths. The old direct-launch path is source-fenced after adoption.

`ACTIVATE-001` changes `Legacy -> Migration` while the same atomic candidate also
selects the exact reviewed integration release/admission record and binds the fresh
flat-certificate digest and action-specific authorization envelope. `ACTIVATE-002` keeps that
exact release and changes `Migration -> CapsuleDisabled` only after the all-three
bootstrap and `RuntimeHeld`. `ACTIVATE-003` keeps that exact release and changes
`CapsuleDisabled -> Autonomous` only with fresh provider/resource/source-fence/
review/engineering and separate operator authorization bound to the whole
candidate. There is no reverse or skip edge and no second activation selector.

Every later release switch uses `RELEASE-SWITCH-001..004`, also owned by
`AO-HOST`: stop/drain, validate a new immutable release and Capsule-compatibility
manifest, commit a full-record compare-and-swap candidate, parent-sync/reopen, and
restart. A switch from either `CapsuleDisabled` or `Autonomous` always selects
`CapsuleDisabled`, clears the prior autonomous provider/review/engineering/operator
authorization, and binds any deployment approval only to the new complete record.
Existing reconciliation, reduction, redemption, and settlement remain available;
only a later fresh `ACTIVATE-003` may reopen entry.

`DEV-EPOCH-*` likewise selects `CapsuleDisabled`, binds the new signed device
admission and any changed release/compatibility manifest, and clears all prior
autonomous authorization; it can never carry `Autonomous` or a prior review digest
across a release/device-epoch change. The kernel old-device denylist is boot-volatile.
Prestart reconstructs it only from the durable selected manifest and verifies it
before any voter bytes are read on every process/host restart.

Device-epoch admission has one ordered transition sequence:

1. `DEV-EPOCH-001` stops, masks, and drains every old runtime process.
2. `DEV-EPOCH-002` writes, syncs, and signature-checks the new immutable release
   plus its 4-KiB admission record and builds the full `CapsuleDisabled` candidate
   with fresh device/compatibility/operator evidence and cleared autonomous fields.
3. `DEV-EPOCH-003` verifies the current/inactive-before guard before mutation and
   the unchanged-current/complete-prepared-target guard after sync, then atomically
   exchanges the fixed pair with `RENAME_EXCHANGE`; no new byte or inode is
   allocated.
4. `DEV-EPOCH-004` syncs the selector parent, then reopens and verifies the exact
   selector/release/admission digests.
5. `DEV-EPOCH-005` installs and verifies the boot-volatile kernel old-device
   denylist from the selected durable manifest on every boot/start.
6. `DEV-EPOCH-006` enables voter opens and certificate capture only for that
   process after the verified kernel fence; every restart resets this read gate
   closed. Replica-A publication remains unreachable.
7. `DEV-EPOCH-007` verifies the already-enforced launcher rejects every retained
   release whose digest/epoch differs and verifies deploy tooling rejects
   `candidate_epoch <= active_epoch`; only success makes replica-A publication
   reachable.

Recovery accepts only a fully durable old current/staging mapping (bootstrap
ineligible) or a fully durable exchanged mapping whose parent is synced and whose
selected manifest is reopened exactly. Any mixed/substituted mapping halts. The
selected manifest is durable; the kernel fence is not, so every restart must
reinstall and verify it before the first voter open. A missing, corrupt,
substituted, or rollback selector; an old process that did not drain; or any voter
open before `DEV-EPOCH-005` halts without reading recovery bytes. Thus the old
manifest keeps bootstrap ineligible, while the selected new manifest makes every
old lineage ineligible before A. Bolt never assumes flat.
After an `ExclusiveAccountFence` proves the configured CLOB owner, wallet type,
Safe address and signer set, revokes every Bolt entry path, and proves there is no
other authorized account writer, two complete bounded captures bracketed by
configured finalized Polygon heads must agree. Capture asks for 21 open orders and
11 exact positive positions/redeemable claims, accepts no continuation, preserves
dust, and stores at most 20 order, ten aggregate-risk, and ten settlement slots
keyed by `(account, condition, token)`.

The only new root permitted without a trustworthy Capsule parent is a
`CatastrophicBootstrapCertificate`. Eligibility requires the exact dual-full-media
loss/decommissioning and old-lineage fence above; selected-payload loss while any
original full replica remains eligible can never enter this protocol. Its fixed
encoding contains the sentinel root
domain, exact aggregate payload and `capture_digest`, provider endpoint/schema and
`source_digest`, the two bracketing block numbers/hashes and `finality_digest`,
resolved `config_digest`, ordered recovery-device identities and
`device_set_digest`, the complete unknown-integrity suppression masks/frontier and
`fence_digest`, the exclusive-account proof digest, and the certificate digest
over all preceding fields. It never claims a risk ordinal, booking association,
terminal history, or absence of a latent signed order.

Catastrophic publication has one special crash protocol:

1. Re-capture and validate the exact certificate in bounded memory; until a
   certificate is selected, perform no external effect and emit no canonical
   evidence.
2. Format replica A's fixed files, write the fixed sentinel root to slot 0 and its
   certificate child to slot 1, sync the empty arena, then publish and sync A's
   manifest selecting the child. A is one non-authoritative vote.
3. Re-capture before writing B. Any aggregate, source, finality, configuration,
   device, account-fence, or evidence-fence change invalidates the partial
   certificate; overwrite A with a newly derived certificate before proceeding.
   Otherwise write and sync the byte-identical B slot pair, arena, and manifest.
   A+B are the
   required same-certificate full-replica quorum.
4. Revalidate once more, then write/sync W's certificate record and finally its
   selector. Quarantine effects remain disabled until A, B, and selected W name
   the same certificate and both empty arenas match.

A crash before A publication leaves no vote; after A leaves only one vote; during
B leaves either one vote or the exact A+B full quorum; during W leaves A+B but no
selected witness. Restart repeats the current step after re-capture. A changed
capture can never complete an older partial certificate. The sentinel-root domain
is disjoint from normal parent lineages, and the selected certificate's device-set
digest plus W selector prevents a later stale old replica from forming a competing
quorum; it has one vote and is overwritten from A+B.

The selected payload books the captures as conservative unattributed exposure.
Because lost episode identities cannot be enumerated safely,
`UnknownIntegrityEvidenceFence` disables all canonical evidence and saturates the
entire risk and market state families plus every risk ordinal through both trusted
current and next windows, the maximum pre-loss discovery horizon—not merely states
still observable at the venue. It also permanently saturates every system-state bit
for the lost/current system episode and emits no canonical evidence from captured
aggregates.

While the stable exclusive-account fence remains valid, quarantine may
automatically prepare and send only an exact sell/reduction whose maximum fill
cannot exceed the freshly re-captured unattributed token balance, or an exact
redemption covered by a captured claim. Every action uses the same
durable-before-effect, exact-query, finality, and post-balance proof as normal
recovery; a pre-send re-capture must prove that absolute exposure cannot increase.
An untradeable dust balance remains reserved and reportable until it can be
redeemed. If reduction monotonicity, exact amount, account exclusivity, provider
terminality, or finality cannot be proven, Bolt safely halts that action and keeps
retrying its fixed query state; it never guesses. Entry and canonical evidence stay
disabled, and even a verified zero balance remains `HaltedUnknownIntegrity` until
authenticated exceptional repair. Incomplete pagination, an eleventh aggregate,
or conflict cannot publish a certificate.

If trusted time cannot yet name current and next, the evidence disable remains
open-ended; no canonical evidence is emitted while fixed retry obtains a bracket
and durably installs the two-window fence.

The authenticated exceptional repair cannot infer lost episode ids or unsaturate
an old novelty/ordinal bit. It preserves the OR of every prior suppression mask and
the two-window `UnknownIntegrityEvidenceFence`; the existing trusted window
identity and frontier representations make that fence survive serial wrap.
Previously emitted but now unobservable states therefore cannot recur. Only
genuinely new risk and market episodes may use fresh novelty masks, beginning with
the first exact-slug market strictly beyond that two-window fence after trusted-time
bracketing, normal fresh Gamma discovery, expiry rejection, complete venue
reconciliation, and durable empty barriers. The lost/current system episode stays
fully saturated. If that safe frontier proof is unavailable, fixed retry remains
blocked and reserved.

S3 corruption can lose historical availability, but cannot change recovery truth.
One-voter failure continues quorum-durable reduction and repairs automatically on
return. Fewer than two matching votes or loss of the current full payload initiates
no new effect until quorum/payload returns. Destruction of both full media can
require rare replacement. Throughout, risk increase remains blocked and all
possible exposure stays reserved.

### Lifecycle and rollover

One supervisor owns at most 14 durable desired **lifecycle bundles** keyed by
`(client, instrument, lifecycle role)`. Each bundle owns at most one Polymarket
asset id and therefore at most one Polymarket wire member; book and trade consumers
share that asset subscription rather than multiplying wire members. Polymarket is
capped at 14 wire asset ids, while the global cross-provider wire-member cap remains
64.

The pinned NT Polymarket source exposes no server subscription acknowledgement, so
a successful transport write is not an acknowledgement. Generation-local state is
`Absent -> Requested -> Observed`: `Requested` follows only a successful transport
write, and `Observed` follows only a valid asset-specific full book snapshot, or a
source-fenced sequence-complete baseline, from the current connection generation.
A delta or trade message before that baseline leaves the member `Requested`, marks
the book invalid, and requests one resnapshot; it cannot establish freshness.
Entry requires every required target to be `Observed` and to have a fresh complete
book. These actual states reset to
`Absent` on every reconnect/restart and are never loaded from the Capsule.

Any desired-asset-set change closes and joins the old Polymarket market connection
generation before opening one fresh generation with the exact complete desired
asset set, with every expired asset excluded. A request/observation timeout replaces
the entire generation. The pinned source does provide a per-asset unsubscribe
write, but it is send-only and has no server acknowledgement. Autonomous-profile
source fences make that call unreachable. Whole-generation close/join and exact-
set replacement is therefore the sole lifecycle transition, and old-
generation observations are never reused.
Every provider future and blocking read is source-fenced cancellation-safe and
bounded by its TOML operation deadline. Close/join has the TOML
`market_generation_join_deadline_ms`; expiry keeps entry blocked. If any old task
has not joined by that deadline, the process records only the existing fixed retry
episode and self-terminates. Systemd's bounded restart policy then reconstructs
only the durable exact desired set, so an uncancelable library read cannot retain a
generation forever or create a second cleanup path.
There is one reconciler/client, and connection tokens are reused only after the old
socket and tasks are closed and joined. WebSocket observations establish market-
data readiness only; REST remains authoritative for order, fill, position, and
settlement truth.

Market lifecycle is:

```text
Absent -> ClosedWindowNoAcceptedCandidate
Absent -> Discovered -> Prepared -> Active -> Draining
                   -> Reconciliation/SettlementPending -> Terminal
```

Current and next markets are prepared within the two-market horizon. An instrument
whose venue close/expiry predicate is true is never placed in the WebSocket desired
set. Existing exposure moves to REST reconciliation and settlement ownership; it
does not cause an expired subscription. Role transfer that leaves the desired set
unchanged produces no wire call.

On every process start or reconnect, trusted clock and complete venue market status
are established before any WebSocket generation opens. The supervisor durably
removes every bundle that expired during downtime and transfers any exposure to
REST reconciliation/settlement first. If time or venue status is unavailable, the
actual set remains empty and entry stays blocked; a stale durable desire is never
sent to the provider.

Each configured market lane supplies a `u64` serial window index derived from
immutable market metadata and the configured cadence. It is retirement metadata,
not episode identity. Serial comparison uses RFC-1982-style modular ordering only
for adjacent transitions and the two-window horizon; neither numerical maximum nor
elapsed-distance subtraction has authority. Wrap `MAX -> 0` is an ordinary adjacent
transition and is explicitly tested.

For each lane/serial, Bolt derives one exact discovery slug from the TOML template,
lane, and trusted window, then queries Gamma with `slug=<exact>`, `limit=2`, and
`offset=0` under fixed body and item caps. Exactly one result is accepted only when
it supplies the complete `GammaMarketBinding`: Gamma id, condition id, question id,
exact slug, trusted open/close window, negative-risk mode, and ordered exactly-two
`(outcome_index, normalized_outcome, clob_token_id)` entries. A single matching
response that lacks either token id remains `DiscoveryHydrating` in the one fixed
retry slot and creates no episode. The first complete response binds the tuple
durably before any lifecycle bundle or episode is created. Every later response
must equal it byte-for-byte after the one reviewed normalization; any mutation,
third/missing outcome, reordered index, cap overflow, two results, or wrong
identity blocks the lane without changing the already bound `EvidenceEpisodeId` or
any novelty bit. Slug/window/timestamp-bearing metadata is lifecycle validation,
never an evidence-key component. Only a reviewed transition to a genuinely new
condition/market id after the durable empty barrier creates a new episode. Zero remains absent before close; after trusted time is
beyond the window close it may commit
`ClosedWindowNoAcceptedCandidate` without creating an episode, evidence, or risk
ordinal. This state proves only that Bolt may skip new risk after trusted close; it
does not prove that a market never existed. Bounded account/order/risk capture
remains authoritative for any existing exposure.

Long-downtime recovery never visits missed windows. In one fixed attempt it samples
trusted time, runs at most two exact-slug Gamma queries per lane—one for the current
window and one for the next—and individually reconciles the at-most-20 retained
orders, ten risks, and ten settlements. It may
commit `FrontierRebased` directly before the trusted current window only when no
retained exposure belongs to a skipped window. It marks skipped windows `Unrun`
without episodes/evidence and saturates the current active novelty/ordinal masks.

The entire attempt must finish within the TOML `recovery_fence_lease=30s`; immediately
before commit a second trusted-time sample must still select the same current/next
windows and every Gamma response must remain within that lease. Expiry retries the
same fixed attempt state. Work is at most those two exact-slug queries plus the
already bounded 20 order queries, ten risk queries, ten settlement queries, and two
time samples for the single lane—independent of downtime.

The rebase does not compare elapsed serial distance and is valid across any number
of `u64` wraps; the trusted current serial is installed only after the old two-
window slots pass durable empty barriers. Old canonical ids are rejected by their
monotonic venue-terminal/expiry status, so wrap cannot reactivate an old episode.

During normal continuous operation, the fixed per-lane
`retired_through_window_serial` advances only across contiguous
`Terminal | ClosedWindowNoAcceptedCandidate` states. `FrontierRebased` is the only
nonadjacent
transition and requires the stronger account/discovery/empty-barrier proof above.
Any rediscovered canonical identity retired by either transition is rejected before
episode/evidence construction. Exactly one accepted identity per serial plus the
300-second minimum proves at most 288 market episodes per day regardless of
discovery churn. The current market's ten-bit admitted-risk mask is non-replenishing;
health evidence uses its fixed 64-state system registry and does not reopen a new
evidence episode after recovery.

Market data is coalescible. Execution/account events are not: overflow latches
entry admission closed and triggers a bounded authoritative reconciliation. Delta
overflow invalidates the book and requests one snapshot. Cache eviction requires
zero subscription, order, position, exit, settlement, and reconciliation leases.

Settlement stays in a finite in-place state machine with capped backoff forever:

```text
Pending -> Due -> Prepared -> InFlight/Unknown -> Observed
        -> DurableBooked -> Terminal
```

Attempt count and delay saturate; no attempt objects or terminal "gave up" state
accumulate. Dependency recovery resumes it automatically.

Redemption uses the Rust-native Polymarket relayer SAFE flow; it never shells out
or introduces a second settlement authority. Its deterministic action identity is
the tuple `(chain_id, wallet_type=Safe, safe_address, safe_nonce, target,
calldata_hash)`. For both market modes, the target exposes the current V2 collateral
adapter's inherited external four-argument
`redeemPositions(address, bytes32, condition_id, uint256[])` ABI. The complete
dummy first/second/fourth arguments are manifest-bound and source-fenced as ignored
by that exact adapter revision; any source change that reads them blocks the
profile. The standard `CtfCollateralAdapter` internally derives both current CTF
balances, redeems the binary partition, and wraps released USDC.e into current
pUSD/PMCT collateral. The negative-risk target is the current
`NegRiskCtfCollateralAdapter`, whose internal override derives the wrapped-collateral
position ids and exact current balances, calls the legacy
`INegRiskAdapter.redeemPositions(condition_id, amounts)`, then performs the same
USDC.e wrap. Bolt never targets that internal two-argument ABI directly.

Because the reviewed external ABI redeems current balances rather than calldata
amounts, the Capsule stores the exact two-balance pre-state and holds an exclusive
condition mutation lease. Immediately before all-three send authorization it
re-queries and requires the same balances; a pre-send change aborts and reprepares.
After dispatch, any intervening finalized balance change is reconciled from exact
logs/post-state under the same prepared Safe body, never guessed or converted to
floating point. A generated provider manifest binds target, dummy values, ABI,
internal CTF/neg-risk path, collateral, and output asset; resolved TOML must match
or redemption blocks.

All Safe effects share one Capsule-owned account-global `SafeNonceLane`; the
per-condition mutation lease alone is insufficient. Ten settlement claims may be
pending, but only one action may own the current Safe nonce. Before the original
action is signed, the lane reserves fixed capacity for both its bounded relayer body
and one same-nonce fence body. No other condition can prepare a Safe effect until
the lane reaches a durable terminal/empty barrier.

Before any relayer write, Bolt encodes and signs the complete bounded original
body, including the Safe transaction and nonce, and commits its exact bytes,
digest, and action identity in the settlement slot and global lane. Replay uses the
same nonce and byte-identical body. The response's exact relayer `transactionID`,
when known, is stored in place. Recovery queries that id and independently queries
the Safe nonce/transaction hash, Polygon receipt and expected logs, and exact post-
balance or claim state; an account-wide relayer history scan is forbidden.

Relayer `NEW`, `EXECUTED`, and `MINED` are unresolved. `CONFIRMED` is a terminal-
success candidate only after configured Polygon finality, compatible receipt/logs,
and the exact expected current-collateral post-balance/claim state prove
redemption. `FAILED` and `INVALID` are relayer observations only; they cannot make a
still-signed Safe transaction cryptographically unusable.

When Bolt must abandon an unresolved original, it signs one deterministic
same-nonce fence transaction. Its manifest-bound inner Safe call invokes the exact
Safe proxy's side-effect-free `nonce()` getter with zero value and source-fenced
Safe implementation, guard, module, fallback-handler, operation, and signature
semantics. The complete fence body is committed as
`SafeNonceFencePrepared`; one conservative `SafeNonceFenceMayHaveStarted` commit
precedes its relayer syscall. The relayer capability contract must accept the exact
explicit nonce and competing same-nonce request; missing support blocks the profile.

Exactly one of the two signed bodies can consume the Safe nonce. If the redemption
wins, finalized Safe execution, adapter logs, and exact claim/post-balance prove
success. If the fence wins, its finalized Safe execution, nonce advance, and
unchanged exact claim/post-balance prove `PermanentlyFencedNoEffect`; the original
body is then cryptographically unusable. Any other nonce consumer, code/config
drift, conflicting hash, or unexplained post-state is integrity halt. Only after
the winner and post-state are quorum-durable may the lane pass through its empty
barrier and the claim be released or prepared at the next nonce. Outages retain one
fixed retry owner and resume automatically; S3 is irrelevant.

The relayer origin, exact post/query path templates, body/item limits, finality,
and retry timing live in the same TOML network/settlement sections. Relayer API
credentials are one grouped SSM-only credential set resolved by the Rust SSM
client; environment, shared-file, CLI, and alternate secret sources are invalid.
The already-reserved settlement lane and retry owner carry the workflow, so adding
the relayer and Polygon RPC owner rows does not add a live socket or retry slot.

### Host containment

All runtime limits are materialized from one TOML schema. The deploy renderer uses
those values for systemd, mount/project assignments, and a dedicated journald
namespace. Recovery replica A, recovery replica B, the recovery witness, reports,
journal, releases, the sealed legacy inventory, and system-mutable state form seven
closed project classes with hard byte/inode limits
on every filesystem they actually occupy. Recovery A, B, and the witness must
resolve to three different device ids. The host provisioner enables and verifies
project-quota support; the current `defaults,nofail` mount is incompatible until
that change lands.

For each of at most four configured devices, prestart/admission requires:

```text
f_bavail - sum(project_hard_limit - current_usage) >= max(applicable_class_byte_floors)
f_favail - sum(project_inode_limit - current_inodes) >= 65,536
```

Remaining project claims on the same device are summed once and different devices
are checked independently. `byte_floor(d)` is the maximum applicable class floor,
never their sum: 10 GiB when any recovery or data class is present, otherwise 2 GiB
for a root/log-only device. `inode_floor(d)` is 65,536 exactly once per device.
Thus colocating root/log with data keeps one 10-GiB byte floor and one inode floor.
This reserves every future allowed write rather than checking current use. Quota,
mount, device-group, inode, allocation, and unit/TOML drift fail closed.

The sealed legacy class includes raw capture, catalog, NT state/cache, four recovery
inputs, directories, block rounding, and xattrs: at most 2,684,354,560 allocated
bytes and 16,384 total inodes. Exact other inode maxima are 16 per full recovery
replica, 8 witness, 4 reports, 256 journal, 49,152 releases, and 65,536
system-mutable. Each recovery device therefore needs
10,816,061,440 bytes and 65,552 inodes cold. A data device
holding legacy and reports needs 13,438,550,016 bytes and 81,924 inodes during
migration, then 10,754,195,456 bytes and 65,540 inodes after migration. There is no
migration-scratch project or derived local file. Root/log
hosting the witness needs 5,435,883,520 bytes and 180,488 inodes. Colocation sums
project hard claims but applies only the maximum applicable byte floor and one
65,536-inode floor per device rather than using a global total.

The post-migration mutable host-disk peak is 3,462,463,488 bytes and the
migration/quarantine peak is 6,146,818,048 bytes. The added 1-GiB/65,536-inode
system-mutable project is the only persistent write surface outside the named Bolt
classes. The root image is read-only; `/tmp` and `/run` are separately bounded
256-MiB tmpfs mounts with 16,384 inodes each. Units use `ProtectSystem=strict` and
only the generated registered paths are writable, so an omitted daemon or path
cannot consume a floor.

Every queue has both an item cap and a byte cap. WebSocket frames and HTTP bodies
are rejected before unbounded parsing. Spawns, sockets, files, and cache entries use
budgeted wrappers; the pinned NT boundary and source fence reject bypasses. Raw
runtime capture is forbidden in the autonomous profile and is bounded with a
single capture-gap state in other profiles. File logging is disabled; the dedicated
journal rotates at its fixed cap and retry logs are state-transition-deduplicated.

Alerts reuse the fixed 64-state system-health registry. One latest-state delivery
bit per id, one prepared message, one Rust-native in-flight send, and the named
alert retry owner replace an event queue. Transport failure never blocks reduction
or grows history; when it returns, the worker sends the latest active/clear state.
The generated unit uses `Restart=always`, a constant TOML-owned restart delay, and
`StartLimitIntervalSec=0`, so any number of crashes retries without an operator or
growing restart state. The dedicated journal's TOML-owned rate limit and project
quota contain the corresponding log storm.

Retry ownership is one fixed 96-entry table, partitioned into 64 recovery owners
(20 order/action ambiguity, ten settlement, ten risk reconciliation, 16 client
connection/authentication, and eight essential system owners, including replica
repair) and 32 ordinary
owners (14 lifecycle/book and 18 closed-registry background owners). Each candidate
risk acquires four future recovery owners—two action/order, one settlement, and one
reconciliation—before entry. Backoff mutates one entry and never creates attempt
history.

At startup the main process allocates/touches a 512 MiB recovery arena and owns a
separate exact 512 MiB physical ballast claim, with swap disabled. The arena serves
only fixed recovery allocations. The ballast invariant is always
`touched free ballast + full active reviewed reservations + locked reserve = 512 MiB`.
A typed protected non-arena acquisition atomically reserves its complete reviewed
page-rounded maximum before create/open; a later measurement may only prove
`actual <= reservation` and never resize or credit the claim. Release closes and
joins the owner before retouching the same pages. Long-lived claims therefore do
not make readiness demand impossible full retouching. The maxima are 128 one-MiB
resident-stack claims, 30 protected sockets at 6,291,456 bytes each (16 WebSocket
generation owners, 12 protected HTTP lanes, and two DNS sockets), and 213,909,504
permanently touched bytes. The native-thread registry and effective main-unit
`TasksMax=128` forbid a 129th native thread; the 512 async-future cap is a separate
runtime-object bound and grants no thread or stack capacity. Stack guard/VMA/page-
table metadata and recovery/config page cache are charged once in `N_main`, not in
ballast. Entry requires the equality plus `actual <= reserved` for every active
claim.

Each socket reserves this complete `C=6,291,456` before `socket` or `connect`:

```text
TLS plaintext/ciphertext       8 * 131,072 = 1,048,576
protocol codec/control        16 *  65,536 = 1,048,576
dial/handshake state          16 *  65,536 = 1,048,576
effective Linux receive        1 *1,048,576 = 1,048,576
effective Linux send           1 *1,048,576 = 1,048,576
socket/TCP objects             16*  16,384 =   262,144
sk_buff/fragment objects       64*   4,096 =   262,144
backlog/retransmit metadata    32*   8,192 =   262,144
optmem/ancillary/port state    16*  16,384 =   262,144
total                                         6,291,456
```

Four ordinary sockets reserve the same `C` from the ordinary pool. The post-create
inspection must find every component inside its reservation. Linux socket-buffer
doubling, receive/send maxima, autotuning, backlog, retransmit, fragment, optmem,
and library resizing are pinned by the signed AMI/TOML profile and verified from
effective sysctls/socket options. On close, the signed-kernel charge map partitions
every retained byte by its effective owner before `C` is released or ballast is
retouched. Main-cgroup-charged socket residue retains a disjoint
`N_main.net_retained` claim until `memory.current`, `memory.stat sock`, and the
generated object counters prove uncharge. Only root/unmanaged residue transfers to
`K_host`. Route/neighbour, DNS/UDP residue, and TLS-session cache follow the same
charge-owner rule. A close can never make the same physical byte disappear from
one claim before it is owned by another.

Ordinary allocation/acquisition has one hard 2 GiB pool and cannot use either
recovery mapping. A separate 512-MiB main-overhead claim is generated, never
measured into existence. With coefficients taken only from resolved TOML, the
exact-head ELF/DSO build manifest, and the signed kernel/AMI manifest, it proves:

```text
N_live = native_stack_guard_VMA_page_tables(128)
       + rounded_exact_head_ELF_PT_LOAD_and_DSO_closure
       + loader_and_all_other_VMA_metadata_and_page_tables
       + allocator_arena_and_metadata_rows
       + declared_runtime_object_rows
       + recovery_and_config_page_cache_rows
       + main_cgroup_retained_network_rows
       + process_attributed_non_network_kernel_object_rows

N_migration = migrator_ELF_DSO_loader_VMA_page_tables
            + migrator_allocator_and_runtime_object_rows
            + migration_config_page_cache_rows
            + migration_process_attributed_non_network_kernel_rows

N_main = max(N_live, N_migration) <= 536,870,912
```

Every term has a generated row, unit, count, coefficient, source-manifest digest,
ownership tag, and subtotal. Resident native-stack pages are ballast-owned; active
socket userspace/kernel bytes are `C`-owned; retained network state is partitioned
between `N_main.net_retained` and `K_host` by the signed-kernel charge map;
allocator payloads are ordinary/recovery-pool-owned. Those bytes are
excluded from `N_main`. The generator rejects missing/duplicate ownership and
mechanically proves the resolved sum is at most 536,870,912 bytes. Missing or
unbounded mappings, allocator classes, page-table levels, runtime objects, kernel
charges, or page-cache ownership block the profile. Runtime RSS/cgroup observations
are drift checks only. The operational ceiling is 3.5 GiB and a further unallocatable
256-MiB guard separates it from `MemoryMax=3.75 GiB`; `MemorySwapMax=0` and
`MemoryMin=1.5 GiB`. The generated main and migration units also set effective
`LimitSTACK=1,048,576`; every native-thread builder requests exactly that resident
stack maximum and no provider/library path may create a larger or unregistered
stack mapping. Admission reserves every remaining typed/ordinary growth
against the operational ceiling, while the generated formula and maximum-state
verifier prove the overhead class stays within 512 MiB. The S3 SDK runs as the same Rust binary in a
non-authoritative archive-worker mode with a 192-MiB operational ceiling,
`MemoryMax=256 MiB`, a separate 32-future async semaphore, native-OS-task
`TasksMax=16`, `LimitNOFILE=64`,
one object, one live connection, no idle connection, redirects and SDK retries
disabled, and read-only access to prepared bytes. Maximum-object upload and cleanup
listing must complete inside the operational ceiling; OOM containment alone does
not pass verification.

AWS credentials use one explicit IMDSv2-only provider. Environment, shared-file,
ECS, web-identity, process, and default provider-chain discovery are disabled.
Both processes permit one 64-KiB IMDS response, one credential generation, one
TOML-owned expiry/refresh timer, and no SDK retry; Bolt's fixed auth owner retries.
The intended resolved TOML generates one closed `NetworkFootprint`: 18 HTTP owner
rows with 17 populated, including alert, Polygon RPC, and relayer; 16 WebSocket
generation-owner rows with 11 populated; a 19-origin capacity with 18 populated;
and 18 DNS/TLS rows with 17 populated. The audited repository currently has 14 HTTP
owners, 15 origins, and 14 DNS/TLS rows: alert, RPC, and relayer are not all
implemented. Autonomous startup remains disabled until generated validation reaches
the intended counts and the unused row in each registry remains explicitly empty.
Live caps remain 12 protected HTTP, four ordinary HTTP, and
two DNS sockets. HTTP is 1.1 with zero idle connections, redirects, proxies, or
library retries; dials are serial and close/join precedes redial. A WebSocket owner
therefore has at most one physical socket, not two overlapping generations. Main
physical sockets are capped at 34: 16 WebSocket + 12 protected HTTP + four ordinary
HTTP + two DNS; the protected subset is 30. Every connect, client construction,
network-helper spawn, and raw socket must consume its declared generated row before
open, and an unknown row blocks pre-open. The archive worker separately has exactly
two origins (S3 and IMDS), used sequentially through one live HTTP/1.1 connection,
zero idle/redirect/proxy/library retry, and one credential generation.

`NetworkLifetimeFootprint` bounds closed-socket residues as well as live sockets.
For every generated owner `o`, TOML supplies token-bucket capacity `b_o`, refill
rate `rho_o`, live concurrency `c_o`, minimum dial interval `delta_o`, and healthy interval `sigma_o` before
backoff/bucket state may reset. For any kernel retention window `T`, the mechanical
dial bound is

```text
D_o(T) = min(b_o + ceil(rho_o*T), 1 + floor(T/delta_o))
retained_o(T) <= c_o * (ceil(T/delta_o) + 1)
TIME_WAIT_max = sum_o D_o(T_time_wait)
FIN_max       = sum_o D_o(T_fin)
orphan_max    = sum_o D_o(T_orphan)
conntrack_max = live_tcp_max + sum_o D_o(T_conntrack)
ephemeral_max = live_tcp_max + TIME_WAIT_max + FIN_max + orphan_max
```

`ephemeral_max` must fit the signed-AMI ephemeral-port interval after its configured
reserve. TIME_WAIT, FIN/orphan, conntrack, ephemeral-port, socket backlog,
retransmit, fragment, and optmem sysctls/caps must equal the manifest. BTF-derived
per-object bytes, route/neighbour entries, two DNS-UDP sockets, TLS-session cache,
ephemeral bookkeeping, fixed owner-bucket state, and all other closed-connection
residues generate `K_network`, with each row tagged `main-cgroup` or
`root/unmanaged`; a reconnect storm cannot allocate outside it. Before a live
socket releases its `C` claim, each retained object is claimed by
`N_main.net_retained` or `K_host` according to the signed-kernel charge map.
Main-cgroup ownership is not released until cgroup counters prove uncharge.
Pre-dial projection covers both domains and blocks before any row, main
operational/hard limit, port interval, or kernel subtotal can overflow.
Stable-reset state mutates one
fixed owner row and never adds attempt history or drops retained ownership early.

The host claim is exactly 8,053,063,680 bytes: 4,026,531,840 main,
268,435,456 archive worker, 134,217,728 journal, 1,610,612,736 non-Bolt bounded
services (including the two tmpfs mounts), 268,435,456 user/operator slice,
1,073,741,824 touched sacrificial reserve, and a separate 671,088,640-byte kernel
ledger that no cgroup may claim. The signed AMI proof is:

```text
K_host = pinned_AMI_kernel_base
       + ceil(memtotal_max_bytes / base_page_size) * BTF_sizeof_struct_page
       + cpu_count * per_CPU_kernel_cost
       + sum(configured_device_count[type] * per_device_cost[type])
       + bounded_filesystem_global_and_per_mount_rows
       + bounded_cgroup_global_and_per_cgroup_rows
       + uncharged_journal_and_filesystem_cache_rows
       + global_route_neighbour_DNS_UDP_TLS_cache_rows
       + root_unmanaged_retained_TIME_WAIT_FIN_orphan_conntrack_rows
       + other_manifested_global_unmanaged_rows
K_host <= 671,088,640
```

`memtotal_max_bytes` is fixed at 8,589,934,592 for this profile; a larger observed
host is rejected rather than silently increasing RAM-dependent kernel metadata.
Only cache not already charged to a named cgroup may enter `K_host`. Every
coefficient and cap comes from the signed AMI/kernel/BTF manifest or TOML;
an unknown module/device/object class, missing coefficient, disabled network cap,
duplicate ownership, or subtotal overflow blocks startup. `K_host` excludes active
socket `C`, every `N_main` row, and cgroup-owned payload; the generated resolved
subtotal must be at most the fixed cap. Runtime slab/proc/cgroup observations are
drift checks only. Loss of the automatically restarted touched reserve or
infringement of `K_host` blocks entry.
Startup requires `8,053,063,680 <= observed MemTotal <= 8,589,934,592`; the stopped
4-GiB host is incompatible and must not be started for autonomous operation. A
nominal 8-GiB host is accepted only if its observed `MemTotal` and every rendered
cgroup claim meet this interval; a different RAM class needs a newly reviewed
profile and regenerated `K_host` proof.

Zero swap is a host property, not only a Bolt cgroup property. Startup requires
`/proc/swaps` empty, no enabled swap/zram unit, and effective `MemorySwapMax=0` on
main, archive, journal, migration, system, operator, and reserve slices. The signed
AMI contains a closed enabled-unit/socket/timer census. Its generated hard caps are
512 host tasks (`kernel.pid_max=512`) and 8,192 host file descriptions
(`fs.file-max=8192`), with disjoint effective unit/slice maxima summing to at most
those values: main/migration 128 tasks and 2,048 FDs, archive 16/256, journal
16/256, non-Bolt system 256/4,096, operator 64/1,024, and reserve 4/64. Main and
migration are mutually exclusive. Unknown enabled units, socket activation,
effective-limit drift, or a cap-plus-one attempt blocks autonomous startup.

The same AMI census gives every non-Bolt network owner a fixed concurrency,
minimum dial interval, stable-reset interval, retained-object coefficients, and
backoff row. At most 16 non-Bolt network owners and 64 simultaneous non-Bolt
sockets are allowed; their live and retained state is charged to the system slice
or root/unmanaged `K_host` rows according to the signed-kernel charge map. Combined
main, archive, and non-Bolt pre-dial projection must fit the host port, conntrack,
TIME_WAIT, FIN/orphan, task, FD, memory, and `K_host` caps. Any unmanifested
networking service, socket/timer activation, or retry loop blocks entry.

### Migration, operations, and blast radius

Migration cannot begin while live risk exists. Before stopping the old runtime, it
must complete authoritative venue/chain reconciliation and durably prove zero open
or unknown orders, zero positions (including dust), zero redeemable claims, and
zero pending settlements. Any dependency outage or nonzero fact leaves the old
runtime active and the migration unit unable to start. The one-way stopped-service
migration then revokes runtime access with a dedicated identity and continuing
kernel fence before inventory and holds one exclusive lock through quorum
bootstrap publication.

The fence requires every accepted regular file to have exactly one contained link,
directories to match the exact contained-parent/mount topology, no writable FD or
writable shared mapping in the closed `/proc` process census, clean
data after `fsync`/`syncfs`, immutable inode/parent protection, and read ACLs only
for the migrator. Symlinks, nested mounts, bind aliases, or multi-link files block.
It uses Rust-native `POSIX_FADV_DONTNEED` followed by bounded-window `mincore` to
prove zero resident source payload pages before classification; failure retries
without cutover. Subsequent payload reads are direct I/O and no other manifested
unit can read the sealed roots.

Before sealing the quota, source usage must leave exactly 4,096 bytes and one inode
inside the existing legacy project ceiling. The migrator creates and parent-syncs
one same-parent blocking directory, then freezes a sorted path/type/length/SHA-256
inventory of the `S=2,151,809,024`-byte sealed source. The fixed
`F_total=16,384` path inventory includes that blocker and at most
`F_source=16,383` source-data paths,
each with an at-most-512-byte normalized relative path, using at most four fixed
root directory FDs plus one source-data FD. Each fixed 64-byte path descriptor
contains type, logical length, SHA-256, virtual base, and an index into the already
budgeted 640-byte source-metadata row. That row contains the root index and complete
normalized path, so any semantic descriptor can be reopened deterministically with
`openat2` beneath the sealed root. The exact path-descriptor maximum is 1,048,576
bytes and the source-metadata maximum is 10,485,760 bytes. A missing path, alias,
type change, inventory overflow, or content mutation restarts from the still-sealed
source; no dirty local migration file exists.

Feather, raw capture, catalogs, and NT cache/state are historical validation inputs
only. A #883 exact-byte-egress-allowlisted raw-only family may enter the remote
legacy stream unchanged; every unapproved family remains in the fixed local
quarantine. Only registered recovery/evidence JSONL contributes semantic frames.
Its raw bytes never egress: the migrator emits a classified binary frame whose
length exactly equals the source frame, contains the approved semantic fields and
pseudonyms, and pads the remainder. If the complete classified encoding does not
fit that original length, cutover blocks; it never truncates or emits a raw field.
Each frame is bounded to 2,097,152 bytes, the TOML record maximum is
`N=1,048,576` (the incident has 272), and malformed, unregistered semantic,
`N+1`, oversize, or recovery-conflicting input blocks cutover.

Each semantic frame produces one fixed 40-byte in-memory descriptor: SHA-256 of
the canonical `(stream,episode,state)` key, a `u32` virtual source offset, and a
`u32` frame length. All `N*40=41,943,040` bytes are sorted in memory. Binary search
of the ordered virtual ranges selects the 64-byte path descriptor; its metadata
index supplies the complete root-relative path for one `openat2` direct reread.
Digest order defines canonical order; for every equal digest the migrator retains
one at-most-512-byte reference key, rereads each descriptor once, and compares the
complete key. Different keys halt as a digest collision. Equal keys use the
generated commutative/idempotent join.

With direct-I/O alignment `A=4,096`, a clean generation has at most
`N+2F_source = 1,081,342` source-data opens and reads at most
`W_clean = 3S + 4A*F_source + 2AN = 15,313,780,736` source bytes, including every aligned
head/tail reread. It performs one directory traversal, writes zero local derived
bytes, and never re-enumerates paths per semantic record. Crashes may repeat this
closed amount but cannot create progress state or a larger generation.

Migration uses exactly 134,217,728 bytes from the main ordinary pool while live
runtime is stopped:

```text
33,554,432  aligned direct-I/O input buffer
41,943,040  semantic descriptors
 1,048,576  path inventory
 8,392,704  one legacy object payload+envelope buffer
33,554,432  Feather/raw validation decoder
10,485,760  source reopen/egress metadata (16,384 * 640)
 5,238,784  join, exact-key comparison, and fixed slack
-----------
134,217,728
```

Every sealed source read and equal-key reread uses aligned `O_DIRECT`/`RWF_DIRECT`
into that one input buffer. Preflight derives and verifies `STATX_DIOALIGN`, the
reader rounds physical reads to the required block boundary while honoring the
sealed logical length, and buffered fallback is forbidden. The pre-seal
fadvise/mincore fence plus exclusive migrator read ACL makes source payload-data
page-cache charge exactly zero after sealing. Directory blocks, inodes, dentries,
and xattrs are not called zero: their generated combined
`M_legacy_meta=134,217,728`-byte maximum is derived before cutover from allocated
directory blocks plus signed-kernel per-object coefficients and partitioned by
effective charge owner into `N_migration` or `K_host`; overflow blocks. At most four
root FDs plus one source-data FD are inspected. The workspace
has no spill path, second object buffer, merge run, scratch project, or resumable
local hint. A crash discards only bounded memory and restarts deterministically
from the sealed inventory.

Define `S_egress` as the disjoint sum of length-preserving classified registered
JSONL frames and exact-byte-egress-allowlisted raw-only files; define `F_egress` as
their path count. Unapproved raw families are not represented remotely. Therefore
`S_egress<=S`, `F_egress<=F_source=16,383`, and the actual legacy payload is
`L_actual=S_egress+640*F_egress <= 2,162,294,144` bytes. It is cut into at most 258
deterministic payload positions of at most 8,388,608 bytes, each with one 4,096-byte
envelope. A source record may cross payload positions only when all path id,
source offset, continuation, length, and checksum data needed to resume it are
wholly inside the destination envelope; there is no external continuation state.
Conditional PUT and exact length/SHA revalidation make a crash repeat the same
object. The fixed 258-entry table marks unused suffix entries `Empty`; object count
is `ceil(L_actual/8,388,608)`, with zero when `L_actual=0`. S3 delay retains the
sealed egress source plus fixed local quarantine and one in-memory object only; it
never creates local scratch.

Malformed or conflicting imported history uses one closed classifier. A
`HistoricalOnly` frame or `TerminalAssociationOnly` frame whose every external id
has exact permanent terminal proof is quarantined; if its episode is
reconstructable, migration saturates every possible state bit for that entire
episode. If identity is not reconstructable, the
bootstrap installs `UnknownIntegrityEvidenceFence` through trusted current and next
windows, permanently saturates the current system episode, and emits no canonical
evidence at all until genuinely new risk/market episodes beyond that fence become
eligible under the same fresh-frontier rule. `RecoveryBearingUnsafe` includes any
ambiguity about may-have-started, identity, amount, account, permanent terminality,
current exposure/settlement/capacity, or the independent flat certificate and
blocks cutover. Venue aggregates never downgrade that class; this rule cannot erase
or guess exposure.

Registered JSONL raw bytes are never archived. Their length-preserving classified
binary frames omit raw sensitive identifiers and include only approved fields and
pseudonyms; allowlisted raw-only families are the sole exact-byte stream. Any current-risk
pseudonym needed after cutover is computed once and stored in the Capsule; archive
regeneration therefore does not depend on an old HMAC key. The bootstrap Capsule
freezes only the final inventory/classifier/canonical-set digests plus a 258-entry
length/SHA-256 object table (10,320 bytes), seeds active novelty as
`ImportedLegacyOwned`, and
starts both arenas empty. It builds identical direct-parent A/B pairs on both full
replicas, verifies the preallocated blocker and atomically exchanges it with the
legacy authority using same-parent `RENAME_EXCHANGE`, parent-syncs and reopens the
exact inode/type mapping, then publishes both full manifests and the witness. No
byte or inode is allocated after quota sealing. Restart accepts only the exact pre-
exchange or post-exchange mapping. Runtime activation requires all three votes and
both arenas.

Imported evidence belongs only to the legacy stream. Egress source paths are
retained through every S3 acknowledgement and the bounded all-object revalidation;
unapproved paths remain a separate, permanently bounded sealed quarantine and are
never deletion candidates. Only after `DeletionAuthorized` commits to all three
voters may egress paths be deleted. `LocalEgressDeleted` follows an idempotent
beneath/no-symlink traversal and requires every egress path absent plus its parents
synced; quarantine paths and bytes must still match their fixed inventory. The
legacy 365-day remote clock begins at `DeletionAuthorized`, but remote pruning also
requires `LocalEgressDeleted`; S3 can never disappear while it is still needed to
justify egress deletion.

`AO-MIGRATION` lands only disabled conversion and hermetic fixtures. The later
`AO-INTEGRATION` PR converts every JSONL consumer to the bounded Rust export,
deletes/source-fences the Python migrator, and removes the legacy reader while
activating no production profile. There is no compatibility recovery writer.
Production cutover remains mechanically disabled even after archive, rollover,
host, and integration dependencies are present. Only the separately authorized
`ACTIVATE-001..003` sequence may cross it, after every provider/resource/review
gate is green, the engineering ruling is `AUTHORIZED`, and an operator gives the
separate production approval required by this contract.
Operationally, the Capsule needs one encoder, one publication primitive, one arena
writer, and one inspectable accounting report. A defect in the Capsule can affect
all local workflow truth, so exhaustive transition/join tests and venue
reconciliation are the release boundary; a defect in the arena or S3 cannot change
workflow truth.

## Alternative B: Framed Journal Plus Checkpoint

### Authority and data flow

A checksummed append journal would remain the logical local authority, with a full
copy on each recovery device and the same third-device digest witness/quorum rule.
A fixed-size checkpoint would summarize active state, and journal frames after the
checkpoint would replay it. At journal cap, a new checkpoint and compacted journal
would be published atomically on a two-vote quorum; risk increase would require all
three voters.

### Exact bound and failures

With checkpoint cap `C=1 MiB`, journal cap `J=1 MiB`, and metadata `M=4 KiB`, the
compaction peak is `3C + J + 2M = 4,202,496 bytes` per full copy, plus its fixed
evidence arena. Two full copies plus the 16,384-byte witness give a
71,335,936-byte runtime recovery set. Retaining the bounded 4,325,376-byte
recovery legacy gives 75,661,312 bytes under the same 157,351,936-byte ceiling. All other
memory, retry, FD, task, host, S3, and semantic-rate bounds are identical to
Architecture A.

Torn frames are ignored only after checksum and length validation. Durable crash
boundaries are frame write/sync, checkpoint write/sync, suffix seal, pointer
write/sync/rename, parent sync, and old-file retirement; pre/post states at each
boundary must either replay uniquely or remain entry-blocked. Restart validates the
selected checkpoint and replays at most `J`; ambiguity never scans S3. An S3 outage
fills the same arena and blocks new risk while closure partitions remain.

Legacy JSONL would migrate under the same exclusive stopped-service/open-FD fence
into one checkpoint and framed suffix. Report consumers use the same classified
export interface, so compatibility impact matches Architecture A.

### Cost and blast radius

This design supports a larger or less predictable active model, but Bolt's active
risk is explicitly capped. It adds a journal encoder, replay engine, checkpoint
compactor, suffix sealer, and more operator-visible artifacts. Compaction creates
more durable transitions, tail rules, and two valid representations of the same
logical state during publication. A bug in compaction or replay affects all
recovery truth. It is viable but larger than needed, so it is rejected.

## Alternative C: Fixed-Capacity SQLite Store

### Authority and data flow

One logical SQLite database, fully copied on both recovery devices under the same
third-device digest witness/quorum, would own recovery tables, novelty bits,
capacity tokens, and outbox metadata. Quorum transactions would precede external
effects; risk increase would require all three voters. WAL checkpointing and a
fixed page budget would bound each full copy.

### Exact bound and failures

Set filesystem allocation quantum `Q=4,096`, page size `PageSize=4,096`, database
page cap `Pages=256`, WAL frame cap `Frames=256`, and a 32,768-byte shared-memory
cap:

```text
B_db   = 4,096 * ceil(256 * 4,096 / 4,096) = 1,048,576
B_wal  = 4,096 * ceil((32 + 256 * (4,096 + 24)) / 4,096)
       = 1,056,768
B_shm  = 32,768
B_sql  = 2,138,112
B_one_full_with_arena = 33,595,392
B_runtime_two_full_plus_witness = 67,207,168
B_cutover_with_recovery_legacy = 71,532,544
```

All other bounds equal Architecture A. The implementation would need one exclusive
connection, bounded SQLite lookaside/page caches inside the same recovery-memory
arena, a reviewed `SQLITE_FULL` admission mapping, disabled temp/rollback files, and
a custom VFS enforcing these exact DB/WAL/shared-memory lengths.

Durable crash boundaries are transaction WAL-frame write/sync/commit marker,
database checkpoint page write/sync, WAL truncate, schema publication, and migration
transaction commit. Restart performs bounded WAL recovery; any checksum/page/schema
ambiguity keeps entry blocked and never consults S3. S3 outage behavior and the
fixed evidence arena are unchanged. Legacy data imports under the same exclusive
stopped-service fence in one bounded transaction; reporting uses the classified
export interface.

### Cost and blast radius

SQL transactions are attractive, but operations now include SQLite integrity,
checkpoint, VFS, and schema tooling. The custom VFS/WAL containment and C-library
failure surface are disproportionate for ten risks. A mistake in page accounting,
checkpoint mode, or filesystem behavior can invalidate the claimed bound and all
recovery truth. Migration and operational inspection are also more complex. It is
viable but rejected.

## Tradeoffs and Residual Risks

- Fixed capacities can pause entries earlier than an elastic system. This is the
  intended safety trade: changing a bound requires a reviewed TOML change and proof.
- Capsule commit ordering, provider prepare/send, and S3 acknowledge/free are the
  highest-risk implementation points. Crash injection is a release gate.
- Complete local media corruption cannot be made harmless by another local format.
  The design reconstructs observable exposure and blocks; it never treats S3 as a
  hidden second authority.
- A long S3 outage eventually stops new episodes. Existing positions, exits, and
  settlements retain their pre-reserved capacity.
- Canonical evidence intentionally replaces unlimited raw capture, reducing ad hoc
  debugging detail. The reviewed schema must contain the facts needed for audit and
  reconciliation.
- The NT fork becomes part of the proof boundary. Its exact revision, queue and
  lifecycle behavior, and provider source fences must remain pinned and tested.

These risks do not change the selection. They define the verification contract and
the conditions under which autonomous operation remains blocked.
