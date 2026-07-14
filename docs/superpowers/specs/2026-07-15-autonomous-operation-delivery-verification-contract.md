# Continuous Operation Delivery and Verification Contract

This is a durable delivery contract, not permission to implement. Work may begin
only after the independent Claude Code review checks this package against current
source and returns `APPROVE`. The architecture and invariant tables then freeze;
implementation findings may strengthen them but may not silently weaken them.

No step in this document authorizes deployment, starting EC2, production mutation,
or a live order. A supervised canary requires new operator approval after all
publishable-head gates pass.

## Required Interfaces

The following interfaces are correctness boundaries, not optional implementation
preferences.

### Recovery transaction

One serialized service owns Capsule quorum load, conservative repair, capacity
acquisition, workflow transition, novelty bits, outbox ownership, arena
materialization, and voter publication. Callers cannot write individual recovery
files or mutate a second durable ledger. The two full replicas are copies of one
Capsule graph and one logical arena, not competing authorities; the 16-KiB
digest-only witness cannot reconstruct payload state. The service exposes typed
transitions rather than a general key/value API.

A non-risk-increasing transition commits only after the same child digest and
parent have synced votes from two distinct configured devices, including at least
one full replica. Risk increase and entry additionally require all three voters
and both arena replicas to be identical. No provider effect is sent before the
required durable vote set.

The witness contributes a vote only when its selector checksum is valid and names
one fully synced witness record with the exact candidate digest, parent digest, and
configured witness device id. A missing or corrupt selector abstains; recovery
never infers a vote from either unselected record. A+B may repair the witness, but
one full replica plus an invalid witness selector is not a quorum.

### Prepared provider action

For entry and exit, the pinned NT provider exposes:

1. bounded preparation of the exact signed request and expected deterministic venue
   order id without network I/O;
2. a Bolt Capsule commit of request, phase, and full lifecycle reservation, with an
   entry recorded as `EntryPreparedNotAuthorized`;
3. for entry, a separate recheck of trusted time, exact market and expiry, required
   feeds, typed health, and the already committed capacity against the same
   candidate snapshot, followed by either all-three `EntryAborted` or one final
   all-three `DispatchMayHaveStarted` commit that also stores the finalized block,
   exact request/hash, and final predicate digest before the syscall;
4. authorization for the still-live final-commit owner to make that one syscall;
   every restart from the conservative final state is query/fence-only; and
5. exact-hash reconciliation through the permanent provider terminal certificate.

Repairing a missing witness vote can copy `EntryPreparedNotAuthorized`, but it can
never authorize or send. The recheck must run after repair. A stale or expired
candidate commits all-three `EntryAborted` without a send, and its reservation is
released only after that durable terminal transition. After all-three
`DispatchMayHaveStarted`, a crash first queries/fences the exact id and never
risk-increasingly replays it. A negative query never authorizes replay or release.
The final state is conservative even when the crash preceded the syscall and
remains query-only with the full reservation until the provider terminal
certificate resolves it.

The reviewed recovery chain is bounded and exact: the signed order hash selects a
`ProviderTerminalCertificate`. `Filled` carries a complete sorted unique set of at
most 64 canonical 32-byte transaction hashes, each verified by one sequential
finalized receipt capped at 2,097,152 bytes/4,096 log items, exact exchange/indexed
`OrderFilled`, Polygon V2 `getOrderStatus(orderHash)`, and exact post-state.
`PermanentlyTombstonedNoEffect` is a linearizable, restart/rollback-durable exact-
hash tombstone ordered behind every submit/delay/retry/match/duplicate/preapproval
path and is verified against untouched finalized V2 status and exact post-state.
POST-create, GET-order, and trade schemas are distinct decoders; unknown or cross-
endpoint values fail closed. Absence/404, cancel/not-canceled, elapsed time,
unsigned wire expiration, heartbeat cancellation, and every ordinary status are
nonterminal diagnostics. No account-history or uptime-sized log scan exists.

Current public V2 has no signed expiry, nonce, maker-controlled on-chain cancel, or
documented permanent CLOB tombstone, and its transaction-hash vector has no
reviewed completeness/at-most-64 contract, so its negative capability fixtures
must keep autonomous entry mechanically disabled. Zero allowance is only a temporary BUY
settlement fence because restoring it revives the signature; user pause is not a
solution because it also blocks exits.

Provider projections expose exact decimal or scaled-integer quantities. They return
every positive dust balance and raw redeemable and non-redeemable position, without
`f64` conversion, epsilon comparison, or provider-side dust dropping.

No provider task may prepare and send behind an untracked fire-and-forget boundary.
Settlement and redemption provide separate idempotent prepare/query/replay
contracts. `AO-REDEEM` adds a disabled Rust-native Polymarket relayer/SAFE primitive.
Both standard and negative-risk paths target the current V2 collateral adapter's
inherited external `redeemPositions(address,bytes32,condition,uint256[])` ABI with
manifest-fixed dummy arguments source-fenced as ignored. Standard internally burns
through CTF; negative-risk internally derives wrapped-collateral balances, invokes
the legacy two-argument neg-risk call, and then wraps USDC.e to current pUSD/PMCT.
Bolt never targets that internal ABI. Target, dummy values, internal path,
collateral, ABI, and output asset come only from a source-fenced provider manifest
bound by TOML; any missing or mismatched binding blocks, and no address is hardcoded.
An exact pre-balance snapshot plus exclusive condition lease is revalidated before
send; post-dispatch changes reconcile from exact logs/post-state. Its deterministic
identity is `(chain, wallet type, SAFE address, SAFE nonce, target, calldata hash)`.
One account-global 16,384-byte `SafeNonceLane` permits only one current nonce owner
across all conditions and reserves the original and one same-nonce fence body. It
prepares and signs the exact original before any send; retry uses identical bytes.
Reconciliation queries the exact relayer id, on-chain Safe nonce/hash, receipt/log,
and exact post-state. `NEW`, `EXECUTED`, `MINED`, `FAILED`, and `INVALID` never
release alone. To abandon the original, Bolt durably prepares a same-nonce Safe
transaction whose manifest-bound, zero-value inner call invokes the source-fenced
Safe `nonce()` getter. One of the two bodies can consume the nonce: original wins
only with finalized redemption proof; fence wins only with finalized fence
execution, nonce advance, and unchanged claim/post-balance. The relayer must
conformantly accept the explicit competing nonce or the profile blocks. Any other
nonce consumer is integrity halt. Grouped TOML names the SSM-only credentials. The primitive remains
disabled and non-durable until `AO-CAPSULE` becomes the sole durable authority;
existing settlement bookkeeping is never redemption proof.
The signed relayer request, Safe body, signature, authorization header, SSM value,
and raw provider response/error are non-loggable types. Source fences permit only
fixed redacted identifiers, lengths, outcome classes, and digests to enter a
log/evidence/alert formatter.

The autonomous account is Bolt-exclusive. Current-unresolved capture
requests the configured maxima plus one: 21 open orders and 11 active positions or
redeemable claims, with no accepted continuation. Known `DispatchMayHaveStarted` ids are
queried individually; `EntryPreparedNotAuthorized` proves dispatch could not start.
An extra item, cursor, inconsistent snapshot, or unknown external
writer blocks entry and repair; Bolt does not scan account-wide terminal history.

There is no physical-media provider bypass. A current full-replica two-vote quorum
must durably prepare every reconciliation, reduction, redemption, and settlement
effect. Fewer than two matching valid votes, or two digest votes whose only full
payload is invalid, initiates no new external effect and retries voter recovery in
fixed state. Loss of both full media is catastrophic, not normal recovery. The only
sentinel-root transition is a `CatastrophicBootstrapCertificate` containing the
bounded external-source snapshot and finality digest, resolved-config digest,
replacement-device digest, exclusive-account-fence digest, and the exact A, B, then
witness publication order. A partial certificate is never authority and is
invalidated if the external aggregate changes before all three publications.
Pre-catastrophe lineage cannot vote in the new root.

Only after a stable exclusive-account fence may that certificate install exact
unattributed current aggregates. It may authorize only quarantine reduction to zero
and redemption of those exact aggregates; it never authorizes entry or canonical
evidence. The system remains `HaltedUnknownIntegrity` after bootstrap until a
separately authenticated repair restores trustworthy policy and association state.
No S3 read, venue absence, or operator guess clears the halt.

### Lifecycle ownership

One supervisor durably owns desired subscription leases, retry states, market
lifecycle, cache references, and health. Polymarket has no server subscription ACK,
and a successful transport write is not an ACK. Each of the 14 durable lifecycle
bundles owns at most one Polymarket asset id/wire member; its book and trade
consumers share that member. Per-member generation state is `Absent`, `Requested`
only after the transport write succeeds, and `Observed` only after the first valid,
asset-specific full book snapshot or source-fenced sequence-complete baseline from
the current generation. A delta or trade before that baseline cannot observe the
member; it invalidates the local book and requests one bounded resnapshot.

Every desired-set change closes and joins the old market connection generation,
then opens a fresh generation with the exact complete desired asset set, excluding
expired assets. Timeout replaces the entire generation; correctness never depends
on per-asset unsubscribe. The provider has a send-only per-asset unsubscribe call,
but it has no acknowledgement and the autonomous-profile source fence makes it
unreachable. Polymarket is capped at 14 wire asset ids inside the
64-member global subscription cap. REST is authoritative for orders. Every spawn
and resource acquisition is registered and joined/released through this owner.
All provider operations are source-fenced cancellation-safe and TOML-time-bounded.
Close/join uses `market_generation_join_deadline_ms`; on expiry Bolt keeps entry
blocked and self-terminates so the bounded systemd restart reconstructs only the
durable exact desired set. No stale local task is adopted across process lifetime.

Gamma discovery always requests exact `slug`, `limit=2`, `offset=0`. Missing token
ids remain in one bounded `DiscoveryHydrating` retry state and create no episode.
The first fully hydrated response durably binds a `GammaMarketBinding` exactly
`(gamma_market_id, condition_id, question_id, exact_slug, trusted window open/close,
neg_risk_mode, ordered exactly-two [(outcome_index, normalized_outcome,
clob_token_id)])`. It is lifecycle/discovery state, not evidence episode identity.
`EvidenceEpisodeId` excludes slug, window, timestamp, and transient values; it uses
stable logical strategy/target/venue, non-temporal market/condition/question ids,
the ordered outcome/token binding, and a risk ordinal only when the evidence family
requires it. Slug/window churn cannot reset novelty; only a genuinely new market or
condition id can roll the market episode. A later binding mutation blocks
activation. Zero results after trusted close commit
`ClosedWindowNoAcceptedCandidate`.

### Health and admission

Health is typed state, not a log scrape. The central predicate atomically acquires a
candidate's complete capacity vector and fixed signed request with
`EntryPreparedNotAuthorized`. A separate all-three transition may commit
`DispatchMayHaveStarted` only after rechecking trusted time, market/expiry,
required feed freshness, typed health, and the committed capacity against that
same candidate snapshot and atomically storing the finalized block, exact
request/hash, and final predicate digest before the syscall. Every required Polymarket target must also be `Observed` in the current
generation with a fresh book. Existing-risk workflows consult only their
pre-reserved resource class and the dependency required for their next external
action. Entry requires all three recovery voters and both arena replicas to agree;
existing-risk progress requires a current full-replica two-vote quorum.

### Frozen runtime envelope

Delivery and verification use these exact reviewed maxima from the invariant
contract:

- recovery cardinality `10/20/10/13` for risks/orders/settlements/episodes;
- two full replicas of 33,562,624 bytes each, including one 31,457,280-byte arena
  per replica, plus a 16,384-byte witness: 67,141,632 runtime bytes and
  71,467,008 bytes with retained migration inputs, under 78,643,200 bytes per full
  project, 65,536 witness-project bytes, and 157,351,936 aggregate project bytes;
- main memory 3,758,096,384 operational and 4,026,531,840 hard, with
  `MemoryMin=1,610,612,736`; archive memory 201,326,592 operational and
  268,435,456 hard; host `MemTotal` accepted only in the inclusive range
  `[8,053,063,680, 8,589,934,592]` bytes;
- post-migration mutable host disk 3,462,463,488 bytes and migration/quarantine
  peak 6,146,818,048 bytes, with data-device cold minimum 13,438,550,016 bytes and
  81,924 inodes; there is no migration scratch project or local sort spill;
- exactly 128 protected async owners, derived as
  `10*6 + 16*2 + 8*3 + 12 = 128`, with async tasks separate from a hard maximum of
  128 native threads because ballast reserves exactly 128 one-MiB stacks;
  effective main-unit `TasksMax=128` is the OS backstop. There are 14 Polymarket wire
  asset ids under the unchanged 64-member global subscription cap;
- one generated immutable main `NetworkFootprint` from resolved TOML with caps of
  18 HTTP owner rows, 16 WS owner rows, 19 origins, and 18 DNS/TLS rows; exact
  populations of 17 HTTP owners including alert, Polygon, and relayer, 11 WS
  owners, 18 origins, and 17 DNS/TLS rows; 12
  protected plus four ordinary HTTP live slots, two DNS sockets, at most 34 physical
  and 30 protected sockets, and one physical WS per owner because close/join
  precedes redial. Archive keeps two origins and one sequential live connection;
  all HTTP is HTTP/1.1 with zero idle, redirect, proxy, and library retry, and all
  dialing is serial. Every client construction, connect, spawn, and raw socket is a
  registered row; an unknown path fails before open. A generated
  `NetworkLifetimeFootprint` disables and verifies autotuning, charges effective
  `SO_RCVBUF`/`SO_SNDBUF`, TLS/user buffers, kernel-object multiplicities,
  route/neighbour state and DNS UDP/TLS caches, global
  protected/ordinary dial token buckets, per-owner minimum reconnect and stable
  reset, pinned `TIME_WAIT`, FIN/orphan, and conntrack horizons, and ephemeral-port
  bounds. Retained objects satisfy
  `retained <= concurrency * (ceil(H/min_dial) + 1)`. Each of 30 protected sockets
  reserves full `C=6,291,456` from ballast and four ordinary sockets reserve full
  `C` from Ordinary before open. On close, the signed charge map transfers
  main-cgroup residue to `N_main.net_retained` and only root/unmanaged residue to
  `K_host` before `C` can be retouched. Observation is drift evidence only. IMDSv2
  is the sole AWS credential path;
- the protected FD proof is exactly
  `80 + 136 + 64 + 32 + 64 + 48 + 88 = 512`. Main non-socket and host-kernel
  ledgers are generated as
  `N_main = native-thread guard/VMA/page-table metadata (resident stack pages excluded) + ELF PT_LOAD + pinned DSO PT_LOAD +
  loader/vDSO/static TLS/mappings + VMAs/page-table bound from declared virtual
  mappings + fixed-arena allocator metadata + recovery/config page cache + declared
  runtime/control objects + process-attributed nonsocket kernel objects +
  main-cgroup retained socket rows` and
  `K_host = signed-AMI pinned base/kernel static + ceil(memtotal_max/base_page)*BTF
  struct-page bytes + perCPU + per-device + global network/fs/cgroup state +
  route/neighbour and DNS UDP/TLS caches + uncharged-only journal/filesystem cache +
  root/unmanaged retained socket states`. Their ownership sets are disjoint and each
  generated sum must fit its fixed cap. Every coefficient comes from
  resolved TOML, the build/kernel manifest, or signed AMI manifest; a missing term
  blocks. The 6,291,456-byte socket charge remains valid only when every buffer and
  object summand is mechanically enumerated with no opaque slab; and
- S3 at most 1,261,698 objects and 3,314,119,482,752 bytes, with numeric cleanup
  cursors only, delete batches of at most 64, a 262,144-byte response cap, and no
  persisted opaque continuation token. Any frame-continuation metadata is inside
  the fixed 4,096-byte object envelope and cannot increase payload size.

### Authoritative transition registry

Implementation has one transition authority:
`ci/autonomous-transitions.toml`. It is build input, not operator-mutable runtime
state. Its TOML header fixes `max_rows_including_retired=512`,
`descriptor_bytes=64`, `max_durable_ops_per_row=8`,
`max_prerequisite_kinds=64`, `max_state_kinds=1024`,
`max_restart_rule_kinds=512`, `max_effect_kinds=512`,
`max_fault_set_kinds=512`, `max_durable_op_kinds=255`,
`max_authority_kinds=255`, `max_barrier_kinds=255`, and
`max_flag_bits=8`. Retired rows count against 512; only active rows emit a
descriptor, so the read-only table is at most 32,768 bytes. It is charged to the
already enumerated release ELF rows in `N_main` and allocates nothing per event.
The registry contains exactly these fields:

```text
id
authority
from
to
prerequisite
durable_writes
external_effect
commit_barrier
restart_rule
fault_hooks
test
owning_pr
source_owner
retired
```

`id` is a stable upper-case identifier. An id is never renamed or reused; a
removed edge remains a `retired=true` tombstone that generates no runtime edge.
`authority` names exactly one Capsule, arena, witness, release-selector, S3, or
external provider state machine. `durable_writes` is an ordered list of exact
write/sync/rename/quota/selector operations. `external_effect` is either `none` or
one exact provider/S3/kernel effect. `commit_barrier` identifies the first point at
which the successor is recoverable. `restart_rule` names one closed deterministic
state. More than eight ordered durable operations requires splitting the edge at a
recoverable barrier. Authorities, states, prerequisites, effects, barriers, and
restart rules are closed generated enums, not runtime-interpreted prose.
`fault_hooks` includes every applicable before-write, after-write/before-
sync, after-sync/before-publication, before-effect, and response-lost point. `test`
is one fully qualified test id, `owning_pr` is one node in the frozen PR graph, and
`source_owner` identifies exactly one implementation owner declaration for every
active row.

The runtime representation is exactly this field order and width:

```text
#[repr(C)] TransitionDescriptor {
  id_index: u16, from_index: u16, to_index: u16, restart_index: u16,
  prerequisite_mask: u64,
  durable_op_indices: [u8; 8],
  effect_index: u16, fault_set_index: u16,
  authority_index: u8, barrier_index: u8, durable_op_len: u8, flags: u8,
  reserved: [u8; 32],
}
```

Its alignment is eight bytes and its size is exactly 64 bytes: offsets are
`0,2,4,6,8,16,24,26,28,29,30,31,32`, respectively. Index zero is the generated
`none` value where applicable; every reserved byte is zero. Compile-time size,
alignment, and offset assertions plus generator cap/overflow tests reject the
513th row, a 65th prerequisite kind, ninth durable operation, any index outside
its declared header maximum or integer representation, a nonzero reserved byte,
or a descriptor-length mismatch. The read-only generated table itself is
`[[u8; 64]; active_rows]` in canonical little-endian field encoding; access decodes
one descriptor into the asserted `#[repr(C)]` stack value instead of casting bytes
or retaining a second table. Golden cross-endian vectors prove the exact encoding.
Documentation/test strings remain build-time metadata and are charged through the
exact release artifact, not runtime allocation.

The build generator produces the Rust transition enum, the fixed runtime
descriptor table, legal-edge match tables, and
`docs/generated/autonomous-durable-transition-crash-matrix.md` from this one file.
That generated file is bounded by an opening marker with exact byte format
`<!-- BEGIN GENERATED AUTONOMOUS DURABLE TRANSITIONS registry_sha256=<64-lowercase-hex> -->`
and the literal closing marker
`<!-- END GENERATED AUTONOMOUS DURABLE TRANSITIONS -->`; no content is permitted
outside them. Narrative
adversarial rows that are not state edges remain review requirements in this
delivery contract; they may reference an id as test context but may neither use an
exact registered id as the row key nor duplicate/redefine its pre/post edge.
Generated artifacts contain the registry digest and are
never hand edited. Autonomous transition
methods require the generated enum; sealed durable-
write and external-effect wrappers require the matching generated capability, so a
general key/value mutation or unregistered provider call is not callable. In every
autonomous-reachable target, source fences reject direct
`fsync`/`fdatasync`/`syncfs`, `renameat2`, selector publication, quota mutation, S3
mutation, provider send, Safe relayer send, or other registered effect outside
those wrappers. Before `AO-INTEGRATION`, the only intermediate exception is an
exact unchanged callsite in the temporary legacy-only census; it is excluded from
the autonomous target/reachability closure and cannot gain a new caller.

CI runs five closed checks:

- `transition_registry_bijection`: every generated state-machine edge and only
  that edge has one non-retired registry row;
- `durability_syscall_census`: every autonomous-reachable or already-routed durable
  syscall/wrapper callsite maps to the ordered `durable_writes` of one row; every
  remaining legacy-only direct callsite matches exactly one unchanged temporary
  census row and has no autonomous reachability;
- `fault_hook_coverage`: every applicable boundary has its generated hook and the
  named test observes both sides plus response loss for external effects;
- `matrix_generated_from_registry`: the complete generated file, exact markers,
  and registry digest are byte-current, every active row renders once, every
  retired row renders zero times, and no narrative crash row uses a registered id
  as its key or supplies a second edge definition; and
- `no_unregistered_external_effect`: every provider, S3, kernel-fence, quota, and
  selector effect reachable from an autonomous target uses its registered
  capability; the only temporary legacy-target exception must match one unchanged
  census row and be unreachable from every autonomous entrypoint.

`AO-CAPSULE` lands the registry schema, generator, generated crash-matrix file,
sealed wrappers, temporary legacy census, and Capsule/arena/order/exit/settlement
rows. In that same PR, the design-time registered-id rows are mechanically handled
only for the Capsule/arena/order/exit/settlement ids that `AO-CAPSULE` owns. Every
other design-time id is reserved but non-active until its named owning PR; that PR
atomically adds its registry row, regenerates the file, and removes only its own
exact-id narrative edge row. The design rows are review input before their registry
entries exist, not a second authority afterward. A source fence then rejects an active registered id in
the key cell of any crash-matrix row outside that generated file; invariant/PR/test
references remain legal. Each later issue-bound PR adds its own rows in the same file:
`#763` owns `S3-COHORT-001..008`, `S3-RETRY-001..003`, and
`S3-LEGACY-001`; `AO-ROLLOVER` owns
lifecycle/generation rows; `AO-HOST` owns host-health, restart, and reusable
`SELECTOR-INIT-001..004`, `RELEASE-SWITCH-001..004`, and
`DEV-EPOCH-001..007` rows; and `AO-MIGRATION` owns migration and
`MIG-FENCE-001..006`. `AO-MIGRATION` also predeclares and tests the disabled
production-switch edges `ACTIVATE-001..003`: exact operator-authorized invocation
atomically selects the exact integration release and `Migration` mode only after
the flat certificate; exact three-voter
bootstrap plus `RuntimeHeld` selects the Capsule runtime; and a green provider/
resource/review manifest selects the autonomous profile. Those edges use one fixed
current/staging selector pair: the same 8,192-byte/two-inode combined active-release/
runtime-mode pair used by `DEV-EPOCH-*`, not a second selector. They cannot be
invoked by repository tests, CI, or deploy helpers. `AO-INTEGRATION` wires the already registered disabled
edges but may only prove the closed census; discovery
of a missing row returns work to its owning PR and cannot be patched with an
integration-only alternate path.

All selector edges use the sole mutator and one embedded fixed authorization
envelope. Bolt never holds the operator private key and cannot sign the envelope;
its immutable release contains only the public verification key. Before any stop,
mask, or selector mutation, Bolt emits the exact 256-byte challenge from the
4,096-byte Ordinary union workspace to the operator-controlled signer and continues selected legacy or `CapsuleDisabled` risk
management. One in-memory challenge/response slot, including its deadline, is the
complete bound; the operator invocation owns that deadline and creates no
background retry owner/task/timer/queue. Timeout, rejection, crash, or stale response changes no
authority. After a response, Bolt reacquires the selector lock and verifies it
against freshly captured prestate before the first write. The signature binds the transition id, expected full current/staging
record identities, the selected-current digest, the inactive-before digest, the
complete target core digest, and the generated ordered prerequisite-evidence root.
Before the first inactive write, all pre-digests/identities must match. Immediately
before exchange, the current digest/identities must still match and the inactive
inode must be the complete checksum/signature-valid prepared target; its then-
computed full SHA-256 defines the post mapping and is deliberately not a signature
input. The parent-directory lock is held without interruption across both checks,
the write/sync, exchange, parent sync, and reopen verification or abort; after a
crash the supervisor must reacquire it before resuming. Source fences reject every
other selector writer. Concurrent INIT/RELEASE/DEV/ACTIVATE tests require exactly
one CAS winner; every loser, inactive-before mismatch, prepared-target mismatch,
and stale authorization leaves authority unchanged.

The approval workspace is exactly one 4,096-byte Ordinary union buffer. It is
phase-reused for the full target core, exact 256-byte challenge, exact 512-byte
response, and full final target. Challenge bytes are `16 header + 16 expiry/reserved
+ 96 three hashes + 32 prerequisite root + 32 four u64 device/inode values + 32
Ed25519 key id + 32 zero reserved = 256`; response adds `64 signature + 192 zero
reserved = 512`; unused union bytes are zero. Only the 512-byte response is copied
to a fixed region of the already-reserved selector-mutator stack while the buffer
is zeroed and reuses all 4,096 bytes to reconstruct the target. Encoding tests and
the Ordinary ledger reject a second buffer or any byte/zero-field mismatch.

The first registry PR cannot pretend current legacy source already uses the sealed
wrappers. It creates one temporary hashed
`ci/autonomous-transition-legacy-census.toml`. Every direct durable/effect callsite
that predates the registry has exactly one row containing
`callsite_id, path, symbol, lexical_ordinal, node_kind,
canonicalizer_version, raw_node_sha256, canonical_node_sha256, effect_class,
removal_pr`. `callsite_id` is stable and never reused; `lexical_ordinal` counts
matching AST nodes within the fully qualified symbol, not source lines.
`canonicalizer_version=1` is a repository-pinned Rust-token/AST algorithm whose
versioned test vectors freeze comment, whitespace, raw-string, macro, method-call,
and fully qualified path handling. `raw_node_sha256` covers the exact UTF-8 AST
node and `canonical_node_sha256` covers its canonical token stream. CI rejects a
duplicate/missing id or ordinal, a new row, either digest drift, a canonicalizer
change, a changed owner, or use from a new autonomous module.

The census grants no autonomous-profile reachability. Each row receives a unique
link marker, legacy census modules are excluded from the autonomous Cargo target
closure, and `no_autonomous_census_reachability` proves both that the generated
closed owner/call graph from every autonomous entrypoint reaches no census symbol
and that no census marker exists in the autonomous binary. Unclosed trait-object,
function-pointer, macro, build-script, FFI, or dynamic-library edges fail that
proof rather than being assumed absent. Each owning PR removes its rows as it
routes the call through a registered capability. `AO-INTEGRATION` requires the
census to be empty, proves the autonomous reachability and link-marker sets empty,
and deletes the file in the same PR. Thus the census is a monotonically shrinking
source-fence exception list, never transition authority or a final alternate path.

## Crash Matrix

Every row is a mandatory injection boundary. `Pre` and `Post` are both safe restart
states; `Unknown` is an explicit durable phase, never an inference that an external
action did not happen.

### Capsule publication and repair

| Crash point | Durable possibilities | Deterministic restart behavior |
|---|---|---|
| While building or size-checking successor | Existing same-digest quorum only | Select that quorum; no capacity or effect was authorized |
| During inactive-slot overwrite or manifest publication on first full replica | Old quorum remains; first replica may contain partial bytes or one new vote | Ignore a lone vote and select the old quorum; no provider effect exists |
| First full-replica vote synced, before second vote | One new vote and two old votes | Select the old quorum; later repair the lone voter without lineage growth |
| Same child synced on both full replicas, before witness vote | New A+B quorum; witness still selects the parent or abstains | Select the child for recovery. A risk-increasing child remains unsendable; repairing W copies only the child vote and never authorizes or sends an `EntryPreparedNotAuthorized` request |
| Same child synced on one full replica and a valid selector-selected witness, before other full replica | New A+W or B+W quorum containing current payload; stale full replica remains old | Select the child for non-risk-increasing recovery, keep entry `ReplicaDegraded`, and copy the selected full payload and arena to the stale replica |
| One full replica matches an unselected witness record, or the witness selector is missing/corrupt | One full payload plus no valid witness vote | Treat W as abstaining: there is no quorum, no inferred record selection, and no new effect; wait for the other full replica or repair from A+B |
| During witness record/selector publication | Selector is old/new/missing/corrupt; both full replicas hold the same child | Validate only the checksum-valid selector and its named fully synced record with exact digest/parent/device id. A+B selects the child and repairs W; risk increase remains blocked until W casts that exact vote |
| All three votes and both arenas synced, before caller sees success | Complete child commit | Resume the encoded post-state. `EntryPreparedNotAuthorized` still requires a fresh separate authorize-or-abort transition and cannot send automatically |
| Crash while a later commit overwrites an older inactive slot | Each full replica's selected slot remains intact and at least the prior quorum survives | Select the quorum digest, ignore partial inactive bytes, and repair the interrupted voter |
| Exactly one voter is unavailable during unbounded non-risk-increasing commits | The other two voters share one current digest; at least one is a full replica | Continue only reconciliation/reduction/settlement/evidence/terminal work, block entry, and mutate one fixed repair state |
| Failed voter returns arbitrarily stale | Current two-vote quorum plus a stale record/replica at any lineage distance | Verify device identity, copy the quorum-selected full payload and arena as needed, publish its vote, and reopen entry only after all three voters and both arenas agree |
| A stale replica returns while the former survivor is absent | No digest has two votes | Initiate no new effect; probe in fixed retry state until a current full-payload quorum returns |
| Manifest missing/corrupt within a full replica and its slots prove a direct parent relation | Two adjacent valid local states plus voter records | Prefer any same-digest quorum; without one, apply only the generated closed join under integrity halt and reconcile before publication |
| Direct child contains a not-all-three-voted `EntryPreparedNotAuthorized` | Full replicas may select the exact child, but entry authorization was never reached | Never send it while votes/arenas disagree. Complete all-three publication of the same prepared child, then freshly revalidate and commit `DispatchMayHaveStarted` or `EntryAborted`; repair alone never authorizes |
| Adjacent workflow phases are incomparable and dispatch may have started | Same identity/request digest with uncertain external phase | Encode `NeedsAuthoritativeQuery`, retain maximum exposure/capacity, and forbid replay/terminal release until venue/sink resolves it; preparation-only ambiguity still freshly authorizes or aborts and never becomes query-only |
| A same-digest quorum exists but no valid full payload for that digest | Digest votes cannot reconstruct state | Set `HaltedUnknownIntegrity`; initiate no external effect and wait for an exact valid payload. A witness or venue aggregate cannot manufacture normal quorum authority |
| No quorum; full copies are nonadjacent or any join is not closed | No trustworthy bounded local selection | Enter integrity halt and initiate no external effect. Ordinary recovery cannot import aggregates or create a sentinel root |
| Selector operator challenge is awaiting a signature, rejected, times out, or the caller crashes | Current selected legacy or `CapsuleDisabled` authority is unchanged; at most one volatile 4-KiB union slot exists | Clear the slot, continue existing-risk management, and require a fresh action-specific operator invocation. No stop, mask, selector write, retry owner, task, timer, queue, or authority change occurred |
| `SELECTOR-INIT-001`: after explicit operator initiation, while taking the parent-directory lock and stopping/masking/draining the direct legacy unit | The direct launcher still names the exact legacy release; its unit is running, stopped, or durably masked; the selector is non-authoritative | Keep the selector launcher disabled, repeat the bounded cgroup/process drain, and retain the direct launcher as sole but stopped authority. The restart-enabled host transition supervisor resumes without another operator action |
| `SELECTOR-INIT-002`: while writing/syncing two fixed `Legacy` records and proving same-filesystem `RENAME_EXCHANGE` support on the non-authoritative pair | The direct unit remains masked; neither, one, or both selector records may be durable, and the exercised pair is exactly pre/post exchange | Rebuild only the fixed pair from the verified immutable legacy release, exchange/reopen it under the single mutator lock, sync its parent, and accept no substituted inode/device. The selector still cannot launch |
| `SELECTOR-INIT-003`: while durably replacing the masked direct unit with the immutable selector-only launcher | The exact old launcher or exact selector launcher is selected; both remain masked and no runtime runs | Accept only the exact pre/post launcher inode/digest mapping. Pre resumes replacement; post verifies that `Legacy` selects the same immutable legacy release. A mixed unit or executable direct path halts |
| `SELECTOR-INIT-004`: while verifying/unmasking the selector launcher and permanently fencing direct launch | Selector launcher and exact pair are durable; the unit is still masked or enabled | Reopen the launcher/pair and source fence, then unmask only the selector launcher. Post-state runs the same legacy release with the selector as sole deployment/mode authority; crash resumes automatically |
| `RELEASE-SWITCH-001`: while taking the selector lock and stopping/draining a Capsule runtime | Current selector remains authoritative in `CapsuleDisabled` or `Autonomous`; an old process may still exist | Keep new release unreachable; repeat stop/drain until the cgroup/process census is empty and retain every recovery reservation |
| `RELEASE-SWITCH-002`: while validating the immutable release/compatibility manifest and writing/syncing the inactive record | Current record remains selected; candidate is absent, partial, or a complete `CapsuleDisabled` child whose predecessor is the exact current digest | Reject any candidate carrying prior autonomous authorization or an unbound release/device/review digest; rebuild the one inactive record without starting it |
| `RELEASE-SWITCH-003`: during pre-write/target comparisons and `RENAME_EXCHANGE` | The fixed pair is exactly pre or post; the candidate always selects `CapsuleDisabled` and cleared autonomous authorization | Current/inactive-before mismatch aborts before write; unchanged-current or prepared-target mismatch aborts before exchange. Reopen both names and accept only the exact pre/post inode/digest mapping; mixed/substituted mapping halts |
| `RELEASE-SWITCH-004`: after exchange, before/after parent sync, reopen, and restart | New record may be name-visible but not yet proven durable; old runtime remains drained | Sync/reopen the parent/pair/release/admission/compatibility digests, then start only `CapsuleDisabled`. Existing reduction/settlement resumes; entry requires a fresh `ACTIVATE-003` |
| `DEV-EPOCH-001`: while taking the selector lock and stopping/masking/draining the old runtime | Old selector remains authoritative; an old process may still exist | Keep bootstrap and voter opens disabled; repeat stop/mask/drain until the old cgroup and process census are empty |
| `DEV-EPOCH-002`: while writing/syncing/signature-checking the new release/admission record and full candidate | Old selector remains authoritative; new bytes may be absent, partial, or a complete `CapsuleDisabled` child with fresh device/compatibility/operator evidence and cleared autonomous fields | Reject the staging candidate unless every exact signature/digest verifies; never carry `Autonomous` or old authorization/review evidence; never read replacement voters |
| `DEV-EPOCH-003`: during pre-write/target comparisons and `RENAME_EXCHANGE` of the fixed current/staging selector pair | Durable mapping is exactly pre-exchange or post-exchange; both 4-KiB inodes already exist inside the system-mutable project | Current/inactive-before mismatch aborts before write; unchanged-current or prepared-target mismatch aborts before exchange. Reopen both names and accept only the exact two inode/digest mappings. Old-current keeps bootstrap ineligible; new-current remains pre-voter until parent durability verifies. Mixed/substituted mapping halts |
| `DEV-EPOCH-004`: after selector exchange, before/after parent sync and reopen | New current selector may be name-visible but not yet proven durable | Sync the parent and reopen/verify both selector inodes, release/admission digest, signature, and epoch. Any missing/substituted/rollback value halts without voter access |
| `DEV-EPOCH-005`: while installing/verifying the boot-volatile kernel old-device denylist | New selector is durable, but an initial boot or any restart begins with the voter-read gate closed and old media may still be kernel-readable | Rebuild the denylist only from the selected manifest and verify it before every process's first voter open. A stale device appearing here cannot be read or adopted |
| `DEV-EPOCH-006`: after verified kernel fence, while enabling voter reads | New `CapsuleDisabled` selector is durable and this process verified the denylist; no sentinel vote need exist | Open only manifest-declared replacement identities, record zero opens of denied devices, validate the replacement epoch, and permit certificate capture only. Replica-A publication remains unreachable. A process/host restart returns to `DEV-EPOCH-005`, never to an open read gate |
| `DEV-EPOCH-007`: while verifying retained-old-release refusal and rollback rejection | New epoch remains selected; an attempted old binary/deploy must fail before replica-A publication | Verify the launcher already rejects every release-digest/epoch mismatch and deploy tooling rejects `candidate_epoch <= active_epoch`. Only success enables replica-A publication; no rollback path may restore voter eligibility, the old selector, or old autonomous authorization |
| Both full media are lost, before a catastrophic external snapshot and exclusive-account fence are stable | No trustworthy risk ordinal, association, terminal history, causal proof, or novelty history | Capture nothing as authority; bounded probes may report health, but no order, reduction, redemption, settlement, or evidence is emitted |
| While preparing `CatastrophicBootstrapCertificate` | No sentinel-root vote, or a complete certificate over exact bounded aggregates, source/finality, resolved-config, replacement-device, and exclusive-account-fence digests | Recompute the whole certificate. An aggregate/finality/fence change invalidates every partial result and stale lineage is excluded |
| After certificate vote A, before B | A alone names the sentinel child | Validate the exact certificate and current aggregate/fence digest; if unchanged publish B, otherwise invalidate A and restart with no effect |
| After matching A+B, before witness | Two full replicas contain the exact sentinel child, but catastrophic activation is incomplete | Publish only the matching witness record/selector after revalidation. Runtime and provider effects remain disabled until all three are durable |
| After all three certificate votes, before caller observes success | Exact unattributed current aggregates are the sentinel-root payload | Restart in `HaltedUnknownIntegrity`; permit only Capsule-authorized quarantine reduction to zero and redemption of those exact aggregates. Never authorize entry or canonical evidence |
| Catastrophic quarantine aggregate changes during reduction/redemption | Exact prior aggregate and prepared action remain durable | Reconcile the exact identity and rebuild the bounded aggregate snapshot through normal child commits; never reuse the bootstrap certificate or infer absence |
| Authenticated policy/association repair is incomplete or crashes | Sentinel root remains `HaltedUnknownIntegrity` | Continue exact reporting/reconciliation/reduction/redemption only. Entry and canonical evidence remain blocked until the separate authenticated repair is complete |
| All full slots unusable and venue unavailable/incomplete/over maximum | No complete local or external view | Never assume flat; retain fixed retry/quarantine state, send no new effect, and remain halted without consulting S3 |

The manifest never chooses the maximum generation number. Every logical slot has a
durable empty/reusable generation before identity change; adjacent versions cannot
contain two different identities for one slot. This applies to risk, evidence,
archive, retry, and lifecycle ownership. Any diagnostic sequence is fixed-width and
wrapping-tested; it has no authority semantics.

`UnknownIntegrityEvidenceFence` emits no canonical evidence. It irreversibly
saturates every possible risk and market novelty state family for each affected
episode and for the trusted current and next windows, not merely states still
observable at the venue, and permanently saturates every system-state bit for the
lost/current system episode. None of those bits can clear or unsaturate. Only truly
new risk/market episodes beginning with the first fully hydrated, genuinely new
market/condition id discovered beyond that bounded two-window lifecycle fence may
produce normal evidence, and only after fresh trusted-time and discovery validation.

That bounded fence applies to a recoverable corrupt-history case with a trustworthy
Capsule authority. A catastrophic sentinel root is stricter: it emits no canonical
evidence for any episode and cannot reopen entry or evidence merely by crossing a
market frontier; only the authenticated repair contract can clear its halt.

### Evidence arena

| Crash point | Durable possibilities | Deterministic restart behavior |
|---|---|---|
| Before typed receipt commit | Receipt `Unseen`; novelty clear | Re-evaluate; no record exists |
| During `Unseen -> PendingArena` commit | Old state or complete bounded receipt+novelty | Old means no record; pending is the sole logical production event |
| Pending durable, before materializer selection | Exact receipt and fixed offset; no global frame | Fixed-order materializer derives only from receipt/episode fields |
| During `PendingArena -> Preparing` commit | Pending, or `Preparing` plus global exact frame/coordinates/digest | Pending selects again; preparing writes only the committed frame |
| Preparing durable, before arena write | Full frame in Capsule; fixed target slot owned | Write exact frame to fixed offset |
| During either arena write or before its sync | One or both replicas may contain partial/stale/complete bytes | Validate each checksum; rewrite the exact prepared frame from the quorum-selected Capsule without allocating another slot |
| One arena synced, before the second | Exact frame exists on one full replica | Keep entry blocked; a surviving full-replica+witness quorum may continue non-risk-increasing work and repairs the other arena on return |
| Both arenas synced, before `Preparing -> Ready` commit | Prepared frame plus matching bytes on both replicas | Promote the same fixed slot through the required voter quorum; never allocate another |
| During ready/clear-global-frame commit | Preparing or ready survives | Preparing validates/promotes; ready uploads once |
| Crash after ready | Novelty set and committed slot | Normal archive path; repeated raw state is a no-op |
| One arena/full replica unavailable during a risk transition | Workflow successor plus one fixed `PendingArena` receipt has a full-replica+witness quorum, or neither does | Advance only the safety workflow through that quorum, keep entry blocked, and later materialize the exact canonical state to both replicas without reevaluating input |
| Global full-frame slot remains occupied while other risks close | One prepared frame plus up to 64 receipts in each of ten risk slots | All risks continue through their own fixed receipts; no risk borrows another partition |
| During remote-ack `Ready -> Archived` | Ready or archived survives | Ready re-verifies; archived retains novelty until episode retirement |
| Archived episode before durable empty barrier | Fixed offset may contain stale bytes | Identity cannot change; empty barrier precedes the next episode overwrite |
| Arena checksum mismatch before S3 ack | Owned corrupt historical slot | Quarantine fixed slot, block new risk, preserve recovery truth and closure reserves |
| Any risk/market/system materialization partition full | Existing records and pending receipts intact | Set archive-saturated health and block new risk; no producer can spill into another partition |
| Both evidence arenas are unavailable but a valid current Capsule quorum exists | Workflow successor and one of the 64 pre-reserved compact `PendingArena` receipts fit in the Capsule | Keep entry blocked, commit and perform the exact reduction/settlement, and materialize the fixed receipt automatically after either arena is repaired; evidence bytes are not action authority |
| Current full payload/quorum is unavailable during risk reduction | No authority can durably carry the successor | Initiate no new effect; retain one fixed repair retry state until a valid current full-payload quorum returns |

The logical production point is only the Capsule commit that sets novelty and the
compact `PendingArena` receipt. Its id is `(episode, canonical state)`, so a crash
cannot create a second id. Receipt-to-arena and arena-to-S3 transitions are
materialization and retention, not new production events.

### Entry, order, position, and exit

| Crash point | Durable possibilities | Deterministic restart behavior |
|---|---|---|
| Before candidate capacity transaction | No reservation | Evaluate normally |
| During `EntryPreparedNotAuthorized` publication | Old quorum or an exact request/capacity child with only one/two votes | Provider send remains forbidden; publication reserves capacity but grants no authority |
| A+B hold `EntryPreparedNotAuthorized`, before W is repaired | Prepared state has a full-replica quorum but no all-three entry gate | Repair W to the same prepared child only. Never authorize or send automatically; recheck the same candidate snapshot and then commit all-three `DispatchMayHaveStarted` or `EntryAborted` |
| `EntryPreparedNotAuthorized` durable on all voters, before recheck | Exact request/id and full reserve durable; both arenas identical | Recheck trusted time, market/expiry, all required feeds, typed health, and capacity against the same candidate snapshot |
| Candidate becomes stale or expires before authorization | Prepared request remains unsent with its full reservation | Commit all-three `EntryAborted`; send nothing and release capacity only after that durable terminal commit |
| During final `DispatchMayHaveStarted` publication | Prepared remains selected, or only one/two voters select the final child | Send nothing until the same child—with exact request/hash, finalized block, and final predicate digest—has all three votes and both arenas. Restart completes publication or remains prepared; it never creates another candidate |
| `DispatchMayHaveStarted` durable, before the provider syscall | Exact request/hash, finalized block, final predicates, and full reserve are durable | Treat the dispatch as ambiguous; do not abort or send on restart. Only the still-live commit owner may perform its one immediate syscall |
| During provider syscall or response loss | Exact request/hash and full reserve remain; venue may be absent, pending, filled, or tombstoned | Query/fence only by exact hash. No negative result, timeout, or status authorizes replay or release |
| Exact CLOB query is absent/404, cancel says `not_canceled`, or elapsed time advances | `DispatchMayHaveStarted` and full reservation remain | Continue one bounded exact-hash retry owner. A finalized zero allowance may temporarily fence BUY settlement, but cannot be restored without a permanent certificate |
| CLOB order/trade/heartbeat/cancel status changes | Bounded diagnostic state only | Use it to drive queries, never as recovery authority. Unknown values block; the GET and POST schemas are decoded separately |
| `ProviderTerminalCertificate::Filled` arrives before Capsule commit | Complete sorted unique at-most-64 transaction-hash set plus claimed final quantity | Verify finalized V2 status, every sequential <=2,097,152-B/4,096-log receipt and indexed `OrderFilled`, and exact post-state; item 65, mismatch, duplicate, or overflow remains Unknown |
| `ProviderTerminalCertificate::PermanentlyTombstonedNoEffect` arrives before Capsule commit | Linearizable permanent hash tombstone plus claimed no effect | Verify untouched finalized V2 status and exact post-state; only the all-three terminal commit releases the reservation |
| During either terminal-certificate publication | Prior ambiguous state, a partial child, or complete all-three terminal | Select only the complete all-three child; restart repeats verification and never releases from an individual negative signal |
| Exact finalized fill observed, before Capsule commit | Older exact aggregate locally; newer venue/chain fact | Re-query the same hash/trades/receipt/log and apply a monotonic scaled-integer exposure transition including positive dust |
| Terminal CLOB plus finalized chain/post-state, before Capsule commit | Order may have exact fill, dust, or release | Reconcile raw redeemable/non-redeemable positions and exact fill before freeing any slot |
| During temporary allowance-zero or provider-tombstone request | Prepared protective action may or may not have applied | Reconcile/replay only the exact prepared protective action. It never becomes no-effect proof by itself and never blocks SELL exits |
| Exit decision before `ExitPrepared` | Managed position and pre-reserved closure capacity | Position remains managed; regenerate decision |
| During exit prepare/send/response loss | Exact reducing request absent or accepted | Query/replay by expected venue id; never submit a different concurrent exit |
| Exit rejects/expires/partially fills | Position and current order state retained | Reconcile remaining quantity, retire confirmed terminal order, prepare next bounded attempt in same risk slot |
| Venue becomes flat before local commit | Local position still active | Commit flat/settlement-pending before releasing remaining capacity |
| One voter fails when an exit becomes due | Current full replica plus matching second vote remain; exact reserved exit slot is free | Publish `ExitPrepared` through that two-vote quorum, send only afterward, keep entry blocked, and repair the voter in one fixed owner |
| Quorum/current full payload is lost before an exit send | No durable successor can authorize the effect | Send nothing; retain the managed position and fixed retry state until a current full-replica quorum returns |
| Quorum is lost after an exit send or response loss | Prior quorum contains exact prepared/unknown identity; venue may have applied it | Send no different action. On quorum recovery, query/replay the same identity and retain all capacity until terminal reconciliation |

Capacity is released only by authoritative terminal reconciliation. A timeout,
websocket silence, cache eviction, or retry limit cannot release it.

### Settlement and redemption

| Crash point | Durable possibilities | Deterministic restart behavior |
|---|---|---|
| Outcome observed, before `SettlementPrepared` | Position/settlement pending | Re-read outcome and prepare again |
| During prepared publication | Pending or exact prepared operation | No sink call for pending; resume exact operation for prepared |
| Prepared durable, during request | Sink absent or applied | Query/replay same idempotency identity |
| Applied response lost or before booked commit | Prepared locally; external booking may exist | Query/replay, then commit observed/booked |
| During booked publication | Prepared or booked | Prepared reconciles; booked continues terminal release |
| Booked durable, before release | Durable local settlement association retained | Complete local terminal transition |
| During terminal release | Booked or terminal | No new external effect; resume surviving phase |
| Dependency unavailable for any duration | Same fixed pending/prepared state | One capped retry episode continues forever and resumes automatically |
| Redemption target/collateral/ABI/output-asset manifest binding is missing or mismatched | No valid provider operation can be prepared | Block without signing or sending; retry the one source-fenced manifest/TOML health state. No hardcoded address or alternate ABI is allowed |
| Before account-global `SafeNonceLane` acquisition | Redeemable raw claims remain pending; no condition owns the current nonce | Acquire the sole lane and reserve both fixed body buffers before signing; other claims wait without allocating another nonce owner |
| During `RedemptionPrepared` publication | Pending, or exact `(chain,wallet type,SAFE address,SAFE nonce,target,calldata hash)` plus complete signed body and same-nonce fence capacity | Send nothing from pending; resume only the exact prepared body and retained lane |
| Prepared durable, during relayer send or response loss | Relayer transaction id may be unknown; SAFE nonce/body are fixed | Query by exact relayer id when known and by SAFE nonce/transaction hash, receipt/log, and exact post-balance/claim; replay only the identical nonce/body |
| Relayer reports `NEW`, `EXECUTED`, or `MINED` | Exact redemption remains unresolved | Retain claim and capacity; continue fixed query without terminal booking or release |
| Relayer reports `CONFIRMED`, before Capsule commit | Chain receipt/log and post-state may or may not agree | Commit success only after finalized receipt/log and exact post-balance/claim confirm it; otherwise remain unresolved |
| Relayer reports `FAILED` or `INVALID` | Original signed Safe body remains cryptographically usable until its nonce is consumed | Retry identical bytes or enter `SafeNonceFencePrepared`; never release from the relayer state alone |
| During `SafeNonceFencePrepared` publication | Original and exact deterministic same-nonce fence body/capacity, or only the prior original | Send nothing until the complete fence child is durable; restart rebuilds no alternate body |
| `SafeNonceFencePrepared` durable, during `SafeNonceFenceMayHaveStarted` commit or relayer response loss | Either same-nonce body may win; both exact hashes and full post-state expectation are durable | Query Safe nonce, both transaction hashes/ids, receipts/logs, and claim/post-balance. Never submit a third body |
| Original consumes the nonce | Finalized redemption may have occurred | Commit success only from compatible Safe/adapter logs and exact redeemed post-state |
| Fence consumes the nonce | Original is permanently unusable | Commit `PermanentlyFencedNoEffect` only from finalized fence execution, nonce advance, and unchanged exact claim/post-balance |
| Unexpected body consumes the nonce | Account-global sequencer lost exclusivity | Integrity halt; retain claim/capacity and admit no new Safe or entry effect |
| Confirmed redemption, before settlement booking | Raw claim may already be gone externally while local claim remains | Re-query the exact identity/post-state, commit redeemed then booked, and release only through the durable terminal transition |
| Winner durable, during lane empty barrier | Terminal settlement and current Safe nonce are known; old bodies may remain in provider history | Release the lane only after the quorum-durable empty barrier; then the next pending claim may acquire the new nonce |
| Both full media lost | No trustworthy local settlement id or original association | Ordinary recovery initiates no redemption. Only a complete `CatastrophicBootstrapCertificate` may install at most ten exact unattributed redeemable aggregates, which remain `HaltedUnknownIntegrity` and may redeem only those exact claims |

Autonomous mode refuses a settlement sink without queryable idempotency. It never
uses a terminal "gave up" or booking-error state to make exposure disappear.

### S3 upload and retention

| Crash point | Durable possibilities | Deterministic restart behavior |
|---|---|---|
| Before prepared-object Capsule commit | Terminal market/system/risk episode and ready records only | Deterministically select its one object again; never choose an arbitrary partial batch |
| During prepared-object commit | No object or exact episode, slot list, length, and digest | Resume only the exact committed selection |
| During local object assembly/checksum | Arena unchanged | Rebuild directly into the one fixed worker buffer from selected slots |
| Before/during PUT or response loss | Remote absent or complete; local object retained | Conditional PUT/HEAD same key; verify digest and length |
| Duplicate key with matching content | Remote complete | Treat as success after verification |
| Pending key has conflicting content | Local exact object plus corrupt remote key | Under verified exclusive IAM/bucket state, delete exact key, list/HEAD absent, and conditionally recreate; never free local state first |
| During corrupt-pending-key delete or response loss | Corrupt key present or absent; local exact object retained | Repeat HEAD/delete/list until absent, then conditional PUT the same local object |
| Corrupt pending key absent, before recreate | Remote absent; local exact object retained | Conditional PUT normally; repair state remains one owner |
| Remote verified, before local ack/free commit | Remote complete; local slots retained | Re-HEAD and commit acknowledgement |
| During ack/free Capsule quorum commit | Slots retained or the same free child has a valid two-vote quorum | Retained re-verifies; only quorum-selected free slots may be overwritten, and stale replicas repair before entry |
| Crash with stale bytes in freed arena slots | Bitmap says free | Ignore stale bytes |
| Before first PUT/startup bucket check | No new remote object | Require dedicated bucket and absent versioning status; `Enabled` or `Suspended` blocks |
| `S3-COHORT-001`: `Unverified -> EmptyVerified` | One of 366 initial prefixes is unchecked, or a capped empty result is durable | Repeat `ListObjectsV2(prefix, MaxKeys=1)` at the fixed cursor; unexpected key integrity-halts, empty commits the barrier, and cursor advances only durably |
| `S3-COHORT-002`: `EmptyVerified -> Owned` | Verified-empty slot or exact new owner child | Revalidate the empty barrier and bind the owner atomically; PUT remains forbidden before ownership |
| `S3-COHORT-003`: `Owned -> ReuseBlocked` | Old owner or reuse-blocked child | Preserve all local references and the exact old cohort; new owner/PUT is forbidden |
| `S3-COHORT-004`: `ReuseBlocked -> DeletePrepared` | Old owner and exact fixed key cohort, or durable delete descriptor | Never issue delete without the descriptor; resume the same cohort |
| `S3-COHORT-005`: `DeletePrepared -> Deleting` | Prepared descriptor or deleting state | Begin only the same numeric batch; response loss is handled by exact HEAD/delete retry |
| `S3-COHORT-006`: acknowledged delete cursor advance | Some/all fixed-name keys may be absent; prior or next numeric cursor is durable | Repeat the current idempotent batch until acknowledgement; never infer cursor progress from absence alone |
| `S3-COHORT-007`: `Deleting -> VerifyingEmpty` | All expected delete positions acknowledged, or fixed verification cursor active | HEAD each expected key at the fixed numeric cursor, then run one capped final list; no opaque continuation is stored |
| `S3-COHORT-008`: final empty list -> `EmptyVerified` | Verification cursor complete; prefix empty or unexpected key visible | Commit only the capped empty result. Unexpected key integrity-halts; reuse returns through `S3-COHORT-002` |
| `S3-LEGACY-001`: legacy `EmptyVerified -> Retired` | Legacy prefix is durably empty after its 365-day deadline, durable `LocalEgressDeleted`, and the complete fixed-key delete/HEAD/list barrier | Commit one permanent non-reusable terminal. Restart never returns the legacy prefix to `Owned` or accepts another migration |
| Final list finds an unexpected key | Exclusive-prefix invariant is violated | Enter one bounded integrity halt; do not paginate, spill, or PUT more objects |
| After prune verified, before health quorum commit | Remote cohort absent | Reverify or commit the same current prune health through a two-vote quorum |
| Day tag/ring index wraps after arbitrary downtime | Old slot is `Owned`, `ReuseBlocked`, `DeletePrepared`, `Deleting`, `VerifyingEmpty`, or `EmptyVerified` | Trusted day selects the index directly; ignore elapsed distance and follow only the registered cohort transitions |
| Legacy upload crosses 365 days | Some of at most 258 objects acknowledged; sealed local source retained | Legacy slot is prune-ineligible; continue one bounded upload cursor with no extra object/history |
| Every legacy object acked, during remote revalidation | Local inventory sealed; one `(stream,index)` verification cursor | HEAD every committed key/digest/length under the exclusive prefix lock; any mismatch returns that exact object to pending; no local delete authorized |
| Legacy remote pass complete, before local deletion authorization | All remote objects revalidated; source still sealed | Commit `DeletionAuthorized` and only then start the legacy 365-day retention clock and local deletion pass |
| Unresolved/late-terminal risk still references a ring slot at reuse time | Slot is `ReuseBlocked`; its fixed keys remain the old owner | Block new archive-day admission, upload/ack the final old-owner object, then delete/list/empty before assigning the new day; no extra key namespace |
| `S3-RETRY-001`: retry `Idle -> Armed` | Same prepared remote action plus one due time | Commit one owner/timer; no task or attempt record is appended |
| `S3-RETRY-002`: saturated backoff update | Same owner and earlier/later capped stage | Commit the saturated stage/due time in place; arbitrary failures do not change cardinality |
| `S3-RETRY-003`: retry clear | Same action remains, or its durable successor/ack exists | Clear only after the successor/ack is quorum-durable; response loss keeps retry armed |
| S3 unavailable indefinitely | Remote stops growing; local reaches fixed cap | Entries stop; closure partitions remain; the registered retry state persists |
| S3 returns | Same local records/object | Drain, verify retention, and automatically reopen admission |

Multipart upload is excluded. There are no abandoned multipart sessions to retain.

### Market lifecycle and subscription rollover

| Crash point | Durable possibilities | Deterministic restart behavior |
|---|---|---|
| Restart/downtime crosses a market expiry | Durable desired bundle may now be stale; every member starts `Absent` | Establish trusted time/venue status, durably remove expired bundles and transfer orders/exposure to REST ownership before opening any market WS generation |
| Market discovered, before prepared lifecycle commit | Old desired set | Rediscover; no subscription requested |
| Desired-set commit, before connection replacement | New exact desired asset set durable; old generation may still run | Close and join the old generation, then open a fresh generation with the complete desired set and no expired asset. Autonomous-profile source fences make the provider's send-only per-asset unsubscribe unreachable |
| During old-generation close/join or new-generation open | Old generation is closing/closed, or a partial new transport exists | Close/join whatever generation exists and replace it wholesale from the durable exact desired set |
| Old-generation task ignores cancellation through `market_generation_join_deadline_ms` | Durable desired set is unchanged; entry remains blocked | Record no new owner, self-terminate, and let bounded systemd restart create exactly one generation from the durable set |
| Transport write succeeds, before any asset-specific book message | The written current-generation members are `Requested`, never acknowledged | Keep entry blocked. Send success proves only transport write; Polymarket exposes no server subscription ACK |
| Current-generation delta/trade arrives before a full snapshot or sequence-complete baseline | Member remains `Requested`; book is invalid | Coalesce diagnostics, request one resnapshot, and keep entry blocked; the message cannot create `Observed` |
| First current-generation asset-specific full snapshot or source-fenced sequence-complete baseline arrives | That member may become `Observed`; other requested members remain unobserved | Validate generation, asset id, completeness, and sequence proof; mark only that member `Observed`, and require a fresh complete book for every required target before entry |
| Subscription observation timeout or invalid/stale/wrong-generation message | One or more current members remain `Requested`/`Absent` | Ignore invalid observation, close/join the entire generation, and open a fresh exact-set generation; the per-asset unsubscribe call remains unreachable |
| New market active before old drain commit | Both may be valid inside two-market horizon | Resume drain using one current clock/venue-status snapshot |
| Expiry occurs during disconnect | Remote socket may retain old lease until connection dies | Close/join the generation; reconnect from the exact desired set, which excludes expired assets |
| Old lease removed, before cache purge | Cache still retained | Refcount check and bounded purge later |
| During cache purge | Cache entry retained or absent | Rebuild any desired active entry; never purge an owned risk/reconciliation item |
| Settlement ownership transfer | Draining or settlement-pending state survives | Either still manages order/position or settlement; no gap/duplicate owner |
| Window terminal before retirement-frontier commit | Old frontier and retained episode novelty | Reconcile terminal state again; do not reuse the slot or ordinal |
| Frontier commit before episode-slot empty barrier | New contiguous frontier and old episode slot | Rediscovery at/below frontier is rejected; durably empty the slot before assigning a new identity |
| Exact-slug Gamma discovery returns one fully hydrated item | Query uses `slug=<exact>&limit=2&offset=0` and bounded body/items | Durably bind the first exact `GammaMarketBinding=(gamma_market_id, condition_id, question_id, exact_slug, trusted window open/close, neg_risk_mode, ordered exactly-two [(outcome_index, normalized_outcome, clob_token_id)])`; derive the separate non-temporal `EvidenceEpisodeId` before activation |
| Exact-slug Gamma response is missing either token id | No accepted identity or episode | Enter one bounded `DiscoveryHydrating` retry state. Retry the same fixed query; do not create evidence or a new episode |
| Later Gamma response mutates any `GammaMarketBinding` field or ordered outcome/token pair | Original durable binding remains authoritative | Block activation/entry and report integrity failure; never rebind. Slug/window churn cannot create a competing `EvidenceEpisodeId` |
| Exact-slug Gamma discovery returns two items, cap overflow, or wrong identity | No accepted identity | Block activation and retry the same bounded exact-slug query; never choose an item or create competing episodes |
| Exact-slug Gamma discovery returns zero after trusted window close | Bounded `slug=<exact>&limit=2&offset=0` response is empty but does not prove historical absence | Commit `ClosedWindowNoAcceptedCandidate`: admit no new risk, create no episode/evidence, and advance the bounded adjacent frontier while account/risk reconciliation remains authoritative |
| Serial wraps `MAX -> 0` | Adjacent old or new frontier survives | Use modular adjacent transition only; exact canonical identity and expiry reject ancient replay |
| Restart after arbitrary/multi-wrap downtime | Retained two-window slots plus a bounded rebase attempt; no missed-window list | Under one 30-second freshness lease, query at most the two exact immutable slugs for trusted current and next plus at most 20 order, 10 risk, and 10 settlement current reconciliations; commit `FrontierRebased` only if the closing trusted-time sample selects the same pair, otherwise retry the same fixed attempt state |
| Stop/restart during cleanup | Capsule owns desired lifecycle | Startup recreates only desired leases and joins all old local tasks with process death |

### Legacy migration and one-way cutover

| Crash point | Durable possibilities | Deterministic restart behavior |
|---|---|---|
| Before migration service stops/conflicts old unit | Legacy runtime/JSONL authority only | Old supervised mode remains valid; no migration mutation exists |
| `ACTIVATE-001`: authorized `Legacy -> Migration` full-record compare/exchange/parent sync | The one combined selector pair has exactly the pre mapping (legacy release, `Legacy`) or post mapping (exact reviewed integration release/admission, `Migration`); fresh flat certificate and action-specific authorization envelope are already durable | A comparison mismatch aborts. Before exchange, the selected legacy release remains authoritative but masked while the restart-enabled host supervisor resumes a valid stage; an invalid stage is discarded and the supervisor safely restores selected-legacy risk management with entry still disabled. Post mapping starts only the exact integration migrator; the old runtime cannot reopen. A mixed/substituted mapping halts. Repository tests may exercise only hermetic roots |
| Entry disabled, before authoritative flat certificate | Old runtime remains sole authority with exits/reconciliation/settlement active | Continue exact risk management; begin no migration mutation until two stable finalized captures prove zero orders/positions/dust/claims/settlements. Dependency failure leaves the old runtime active |
| Flat certificate durable, during old-unit stop, cgroup drain, ownership/ACL/immutable revocation, or inode census | Legacy path unchanged; entry stays disabled and authoritative exposure is zero | Keep old runtime masked, install migration-owner `0700` fence, require `st_nlink=1` for regular files plus exact directory topology, no writable FD or writable `MAP_SHARED` mapping, clean fsync/syncfs state, immutable parents/inodes, exclusive migrator read ACL, and fadvise/mincore zero payload residency; otherwise fail before inventory |
| `MIG-FENCE-001`: source reserve verified, during blocker `mkdirat` | Blocker is absent or one exact same-parent 4,096-byte inode exists; source is at most 2,684,350,464 B/16,383 inodes | Reopen by inode/type, remove only an unsynced exact empty candidate, and repeat. Any other entry/substitution or exhausted reserve blocks |
| `MIG-FENCE-002`: blocker created, before/after blocker and parent sync | Exact blocker inode is unsynced or durable | Sync/reopen it, then seal/inventory the complete 2,684,354,560-B/16,384-inode project. No later phase may allocate it |
| Kernel fence and exclusive cutover lock acquired, before bounded inventory | Legacy is inaccessible to runtime/service identities; migration identity is sole possible owner | Restart migration service under the same permissions/systemd fence and scan from byte zero |
| During sealed-path inventory or hashing | Quiescent sealed source plus blocker remain authority; at most the current in-memory path descriptor is incomplete | Restart lexicographic inventory from byte zero. Exactly one blocker plus at most 16,383 sources fit 16,384 64-byte descriptors; no list or sort scratch is written |
| During source metadata/egress derivation | Sealed source remains authority; transient metadata for the current path may be incomplete | Reopen via its 64-byte virtual-range descriptor and 640-byte complete <=512-B root-relative path row using `openat2` and mandatory direct I/O. Classify registered JSONL as length-preserving binary egress, allowlisted raw as exact-byte egress, and unapproved raw as permanent quarantine; raw JSONL/unapproved bytes never egress |
| During registered JSONL semantic parsing or in-memory descriptor sort | Sealed source remains authority; transient at-most-`N` 40-byte descriptors may be incomplete | Discard memory and repeat one clean generation. Binary-search virtual ranges and reopen through at most four root FDs plus one data FD; reread each descriptor at most once, retaining one <=512-B key for a collision group. Enforce `N+2F_source=1,081,342` opens and `3S+4A*F_source+2AN=15,313,780,736` aligned bytes; no re-enumeration/merge/scratch |
| During venue reconciliation | Quiescent legacy unchanged | Retry one bounded reconciliation episode under the lock |
| While zeroing either arena or writing bootstrap slots on either full replica | Legacy unchanged; no quorum; any replica/arena bytes may be absent, partial, or a valid direct-parent bootstrap pair | Under the kernel fence, discard/rebuild both replicas until each has the distinguished zero-parent root and its byte-logical-identical direct child and both arenas match; no live runtime starts |
| While writing bootstrap witness records/selector | Full replicas and arenas may match; W is absent, partial, stale, or selector-invalid | Treat missing/corrupt selector as abstention. A+B rebuild the digest-only witness, sync its exact digest/parent/device-id record, then publish a checksum-valid selector naming only that record; W is never a payload source |
| During final legacy hash and venue reconciliation | Legacy is quiescent and lock-held | Any hash/fact drift invalidates staging; rebuild before path fence |
| `MIG-FENCE-003`: before/during `renameat2(RENAME_EXCHANGE)` | Exact pre-exchange or post-exchange inode/name mapping; exchange is atomic | Accept only source-original/blocker-staging or blocker-original/source-retired. Never infer a missing/mixed mapping |
| `MIG-FENCE-004`: exchange returned, before/after parent sync | Post-exchange mapping may be unsynced or durable | Parent-sync and reopen/verify exact inode/type mapping; no manifest publication before durability |
| `MIG-FENCE-005`: durable mapping, before first Capsule quorum | Old release fails opening legacy path; retired file and validated bootstrap replicas/arenas/witness are preserved | Reverify the blocker-original/source-retired mapping as a publication guard, then publish only the staged exact digest |
| During initial Capsule quorum publication | Fence remains; one/two/all three voters may select the bootstrap child | Resume exact voter publication; runtime activation remains disabled until all three voters and both arenas agree |
| All-three Capsule marker durable, archive lock `MigratorHeld` | Replicated Capsule is sole recovery authority; only migrator may read/archive/delete retired history | Runtime never reads the retired recovery file and uploader remains inactive; prepare the exact lock handoff |
| During `MigratorHeld -> TransferPrepared` | Migrator still holds the exclusive archive writer fence, or a durable transfer descriptor names the runtime/uploader generation | Restart validates the one owner; it either completes the same transfer or remains `MigratorHeld`. No runtime writer activates |
| During `TransferPrepared -> RuntimeHeld` | Transfer is prepared and migrator is quiescent, or runtime/uploader exclusively owns the archive generation | Complete/revalidate the same handoff before activation. There is never a phase with two writers or no resumable owner |
| `ACTIVATE-002`: `Migration -> CapsuleDisabled` full-record compare/exchange/parent sync | Exact pre/post selector mapping over the unchanged integration release; selected three-voter bootstrap, identical arenas, action-specific prestate/target authorization, and `RuntimeHeld` are durable prerequisites | A pre-write or prepared-target comparison mismatch aborts. Pre resumes migration only. Post starts only the Capsule runtime with entry/profile disabled; legacy can never reopen. Mixed/substituted mapping halts |
| `ACTIVATE-003`: `CapsuleDisabled -> Autonomous` full-record compare/exchange/parent sync | Exact pre/post selector mapping over the unchanged release/device epoch; provider/resource/source-fence/review manifest, engineering `AUTHORIZED` ruling, and separate action-specific operator authorization are durable prerequisites and bind the whole candidate/prestate/evidence | A pre-write or prepared-target comparison mismatch aborts. Pre remains Capsule-disabled. Post starts the autonomous profile only after prestart revalidates every digest/gate; any false/drifted gate returns to fail-closed startup, never legacy or migration |
| Imported canonical state recurs before/during legacy upload | Legacy stream owns the classified id; Capsule state is `ImportedLegacyOwned`; arena slot is empty | Treat recurrence as a no-op; never materialize an arena copy, regardless of legacy ack position |
| Crash while a legacy object is assembled or uploaded | Retired source, fixed metadata/digests, and one fixed outbox state | Rebuild the same at-most-8,388,608-byte payload directly from sealed input into the one 8,392,704-byte object buffer; continuation metadata stays inside the 4,096-byte envelope |
| Some legacy objects acknowledged | Complete sealed source and fixed numeric cursor | Stream the next exact object from source; delete no source path and allocate no scratch |
| Every used legacy object acknowledged, before/during remote revalidation | Complete egress source, fixed quarantine, all used-prefix acks, unused suffix `Empty`, and one fixed remote cursor | Recompute both inventories, HEAD every used-prefix object and prove no unexpected unused key by the bounded list/empty checks; any miss/conflict returns the same object to pending; prune remains forbidden |
| Complete remote revalidation, before deletion authorization | Complete egress source, exact quarantine, and verified remote cohort | Commit all-three `DeletionAuthorized`, which starts the legacy 365-day clock, then permit deletion of egress paths only |
| During egress-source deletion after authorization | All upload acks durable; some egress paths remain; quarantine remains complete | Clear immutable protection and delete only remaining egress descriptors, sync parents, and revalidate every quarantine path; never return to upload or delete quarantine |
| `MIG-FENCE-006`: eligible source deletion, during legacy quota reduction | Exact remaining quarantine plus persistent blocker; old or new quota vector is durable | Recount allocation/inodes and reduce the hard quota only to the exact remaining claim, never below 4,096 B/1 inode; blocker identity/type must still match |
| All egress paths absent, before `LocalEgressDeleted` quorum commit | Remote cohort remains protected; quarantine and parent sync may or may not be valid | Recheck egress absence/parent sync and exact quarantine length/digest/allocation, then commit `LocalEgressDeleted` |
| Legacy 365-day clock expires before `LocalEgressDeleted` | Remote cohort is still the egress-deletion proof | Do not delete any remote legacy key; finish/retry egress deletion first |
| During remote legacy delete after both clock expiry and `LocalEgressDeleted` | Used fixed keys may be present or absent; quarantine remains local | Resume numeric delete/HEAD/final-empty verification, then commit permanent `Retired`; namespace is never reused and quarantine is untouched |
| Torn final legacy line | Valid prefix plus bounded corrupt tail | Classify the tail: `HistoricalOnly` or exact permanently terminal `TerminalAssociationOnly` is quarantined and saturates the reconstructable episode/unknown-integrity fence; any possible action/identity/amount/current-authority effect is `RecoveryBearingUnsafe` and blocks cutover |
| Malformed complete/conflicting record | Untrusted legacy history | Apply the same closed three-class table. Never reconstruct active risk or ordinals from venue aggregates: migration requires the independent exact-flat certificate. Missing identity, nonpermanent terminality, may-have-started ambiguity, nonzero fact, or cap overflow blocks |
| Both migration state and venue unavailable | Legacy/fence/staging remain bounded | No new risk; fixed retry resumes when venue returns |

Migration preflight proves the target filesystem supports same-filesystem atomic
exchange. Before inventory, the migration identity takes ownership/mode `0700`,
removes ACL grants, verifies every runtime/service identity lacks DAC-bypass
capability and writable mounts, and proves no process/writable legacy-inode FD
survives. Those kernel permissions remain through publication; the advisory lock
only serializes the sole authorized migrator. It repeats the legacy hash and venue
reconciliation immediately before path exchange. `AO-MIGRATION` lands only after
#763, rollover, Capsule, and host containment but remains mechanically
non-invocable; `AO-INTEGRATION` may remove/source-fence the final legacy code path
and wire only the already registered disabled `ACTIVATE-001..003` entrypoint. No
repository action invokes it. If any fence is unavailable, it makes no authority
mutation and autonomous cutover is blocked; later operator execution can never
make legacy and Capsule simultaneously writable.

## Dependency-Failure Matrix

Every dependency uses one fixed state machine:

```text
Healthy -> Degraded -> ProbeDue -> Recovering -> Healthy
```

There is one mutable retry episode and timer per dependency, not one task, record,
or log per attempt. Operational alert delivery may notify again after a full
healthy transition, but canonical evidence reuses the current market's fixed
system-state registry and never creates a new episode for the recurrence.

A failed dependency may defer one existing-risk effect only when that dependency
is intrinsically required to authenticate, transmit, or prove that exact effect.
The deferral retains the complete pre-reserved state and resumes automatically; it
cannot terminalize work, consume another reserve, or block a different effect whose
own dependencies are healthy. S3, archive, market-data feed, discovery, logging,
and alert delivery are never intrinsic dependencies for exit, reconciliation, or
settlement. SSM is intrinsic only when no already loaded valid credential can
authenticate the exact action; its outage cannot change recovery truth or prevent
an unauthenticated local/quorum step.

| Failure | Entry admission | Existing risk / recovery | Bound and automatic restoration |
|---|---|---|---|
| S3 unavailable, slow, throttled, DNS/TLS failure | Allowed only while complete candidate vector and archive health fit; then blocked | Replicated Capsule/arena remain local authority; exits and settlements use reserved slots and never read S3 | One object, request, retry owner; drain/resume after verified recovery |
| S3 partial success or lost response | Eventually blocked only by local cap | Same local records retained | Conditional PUT/HEAD resolves matching object; mismatching pending content enters one fixed repair state |
| Pending S3 checksum conflict | Blocked only during the fixed repair | Recovery unaffected; exact local object remains | Under verified exclusivity delete/list-absent/recreate automatically; no extra object or retry history |
| Nonempty bucket-version status, IAM drift, or unverifiable ring/legacy delete-list-empty barrier | Blocked | Recovery unaffected; closure evidence and sealed legacy source remain local | No further PUT growth; fixed alert/probe; auto-resume only after exact slot state is valid |
| General network partition | Required feed/auth/venue health blocks entries | Pending actions retain exact identity and capacity; no local terminal inference | Fixed connection generations, queues, retry states; reconnect joins old generation |
| Venue market feed stale/unavailable | Blocked immediately; every required target needs current-generation full-snapshot/sequence-complete `Observed` plus a fresh complete book | Orders/positions use authoritative REST reconciliation when available; otherwise remain pending | One invalid/resnapshot marker per lease; delta-before-baseline cannot observe; timeout closes/joins and replaces the complete Polymarket generation. Autonomous-profile source fences reject every per-asset unsubscribe call |
| Venue command timeout/unknown | Additional risk blocked until reconciled | Retain the exact hash and full capacity; negative/404 never authorizes replay or release. Only the permanent provider terminal certificate resolves an entry ambiguity | At most 20 order and 10 settlement unknown states and one retry owner per dependency. No uptime-sized scan exists; absent provider tombstone capability keeps autonomous entry disabled |
| Polygon RPC, relayer, or redemption post-state unavailable | Blocked for new risk that depends on unresolved capacity | Orders/redemptions retain exact hash or SAFE nonce/body and raw scaled-integer claims; ordinary order/trade/relayer status, absence, and 404 remain unresolved | One bounded retry owner per dependency, no release or replay inference, and automatic exact query continuation on return |
| Venue completely unavailable | Blocked | Cannot transmit externally, but local ability/state/reserve remains; retries continue | No work growth; automatic resumption on venue recovery |
| Market discovery unavailable/ambiguous | No next episode admitted | Current risk continues; terminal markets use known ids and venue reconciliation | One cursor/retry; Gamma uses exact `slug`, `limit=2`, `offset=0`, and bounded body/items. Missing token ids stay `DiscoveryHydrating` without an episode. The first fully hydrated `GammaMarketBinding` is durable; later mutation, two/overflow/wrong identity blocks. `EvidenceEpisodeId` excludes slug/window/time, so churn cannot reset novelty. Zero after close becomes `ClosedWindowNoAcceptedCandidate` without proving historical absence or expanding the two-window horizon |
| Host clock untrusted/regresses | Blocked; no time-derived retirement or freshness | Venue status and monotonic process clock may manage existing risk conservatively | One clock-health state; NTP/venue-time recovery revalidates before entry |
| Project quota, inode, or per-device free-floor pressure | Blocked before candidate crosses boundary | Preallocated replica/arena/witness space remains; every future hard-project byte, inode, and closure token is already reserved. Migration creates no sort scratch | Check at most four device groups. `byte_floor(d)` is the maximum applicable class floor, never their sum: 10 GiB when recovery/data applies, otherwise 2 GiB for root/log. `inode_floor(d)=65,536` once per device; migration/data cold peak is 13,438,550,016 bytes/81,924 inodes and automatic reopen follows quota/mount/sync verification |
| One recovery voter/device ENOSPC, read-only, absent, or fsync-failing | Blocked | A current full replica plus matching second vote may continue quorum-durable non-risk-increasing work; there is no provider bypass | One fixed voter-repair owner probes and resynchronizes it; all three voters and both arenas must match before entry reopens |
| Fewer than two matching voters or no current full payload | Blocked | Initiate no new external effect; retain bounded observations/reservations and probe all voters | Resume automatically when a current full-replica quorum returns; never read S3 or guess |
| One manifest corrupt, same-digest quorum exists | Blocked until repair for entry | Quorum-selected full copy remains authority and may reduce risk | Rebuild the stale voter directly across any lineage distance, then require all-three agreement for entry |
| No quorum, directly adjacent full slots valid | Blocked during closed join | Join possible exposure/ownership within the same `10/20/10/13` layout and reconcile; send no new effect before a new quorum | Direct-parent and exhaustive join-closure proof, then quorum publication |
| No quorum and adjacency/join unprovable | Blocked | Enter integrity halt and initiate no external effect | Fixed voter repair only. Venue aggregates cannot create ordinary authority; a future-risk clear requires authenticated repair |
| Quorum digest lacks a valid full payload | `HaltedUnknownIntegrity` | Digest-only witness cannot reconstruct truth and cannot authorize aggregate quarantine or an effect | Recover the exact payload if it returns. If both full media are lost, only the separate catastrophic-certificate procedure applies; venue facts cannot restore policy or clear halt |
| Both full media/all four payload slots are lost | Blocked | Ordinary recovery captures no aggregate as authority and initiates no effect | A fixed `CatastrophicBootstrapCertificate` may, only under a stable exclusive-account fence and exact bounded source/finality/config/device digests, publish A then B then witness. Partial publication or aggregate drift invalidates it; completion remains halted and permits only exact quarantine reduction/redemption |
| One evidence-arena replica corrupt/unavailable | Blocked for entry | Quorum-selected Capsule receipts and the surviving arena let all ten risks reduce and settle | Repair the fixed offsets from exact prepared frames/surviving replica; no new slot or lineage |
| Both evidence arenas unavailable, valid current Capsule quorum remains | Blocked for entry | All ten risks may reduce and settle using their pre-reserved compact Capsule receipts; no evidence bytes are needed to authorize the effect | Repair either arena, materialize each fixed receipt without reevaluating input, then repair the second; no new slot or lineage |
| Current full payload/quorum unavailable | Blocked | No effect is initiated; no missing authority fact is guessed | Automatic voter recovery resumes exact work if the current payload returns; otherwise the system stays halted. Catastrophic bootstrap is a distinct dual-media-loss procedure, not an ordinary fallback |
| SSM unavailable at startup | Blocked; no alternate secret source | Local/quorum reconciliation continues; only an exact action intrinsically requiring an unavailable credential waits with its full reservation. S3/feed/alert state cannot widen that wait | One SSM retry episode; corrected availability resumes the exact action and startup automatically |
| SSM fails after valid credentials loaded | Authentication health blocks entry | The loaded zeroizing credential generation continues for its exact risk-reducing actions until the provider rejects it; unrelated actions continue and risk remains durable | One refresh generation; a validated replacement swaps atomically and rejected actions resume automatically |
| Malformed/rotated credential | Blocked if no valid generation | Old valid credential may reduce risk until rejection; never replace with malformed value | One refresh state; corrected SSM value auto-restores |
| Provider queue full | Market data coalesces; critical overflow blocks | Execution/account overflow triggers authoritative reconciliation; reduction lane reserved | Item+byte cap; no sender-side hidden queue |
| AWS credential provider unavailable/drifts | Blocked for AWS-dependent admission | Local quorum recovery and venue reduction continue with already valid authentication; no alternate credential source appears | One IMDSv2 generation/timer, one in flight, 65,536-byte response, SDK retries disabled; fixed auth owner resumes SSM/S3 work |
| HTTP/WS owner, origin, DNS/TLS, client, connect, spawn, raw socket, or retry drift/saturation | Invalid config/unknown pre-open path blocks startup; ordinary saturation blocks new work | Generated immutable `NetworkFootprint` preserves 12 protected HTTP, all 16 protected WS rows, two protected DNS sockets, and the 30-socket ballast; Bolt retry table and global dial buckets are sole retry/dial owners | Enforce caps/populations HTTP 18/17, WS 16/11, origins 19/18, DNS-TLS 18/17; four ordinary HTTP slots; physical/protected 34/30; one WS physical per owner; HTTP/1.1 zero idle/redirect/proxy/library retry and serial dial. `NetworkLifetimeFootprint` charges pre-open buffers and post-close `TIME_WAIT`/conntrack/ports; archive remains two origins/one sequential live and any drift blocks |
| Archive worker OOM/crash | Eventually blocked by fixed local archive capacity | Main Capsule/arena authority and closure partitions remain | 201,326,592-byte operational/268,435,456-byte hard cgroup, a 32-permit async semaphore distinct from `TasksMax=16`, and 64 FDs restart through one retry owner; max upload/cleanup must succeed inside operational memory |
| Memory ordinary ceiling/host pressure | Blocked | Charged 512-MiB recovery arena and exact ballast equality remain under `MemoryMin=1,610,612,736`; active typed claims replace touched pages rather than borrowing them | Coalesce ordinary work; restore `touched+active+locked=536,870,912`, the host reserve, kernel gap, and the inclusive `[8,053,063,680, 8,589,934,592]`-byte `MemTotal` predicate before reopening; swap is disabled |
| Async-task, native-thread, or FD ordinary ceiling | Blocked for new work requiring token | 128 protected async owners and 512 FDs remain reserved for reconcile/reduce/settle; native threads cannot exceed the separately charged hard 128 | Join/close completes, health re-evaluates automatically; async saturation cannot spawn an uncharged thread |
| Alert transport unavailable/slow/duplicate | Underlying health state decides; delivery alone does not block | Latest fixed health state remains Capsule-owned; reduction never waits for alert | One prepared/in-flight message and one saturated retry owner; send latest state automatically on recovery |
| Operator authorization signer unavailable/slow/rejects | No production transition has begun and no selector byte changed | Continue selected legacy or `CapsuleDisabled` risk management; keep entry disabled and expose one bounded health state. A later explicit operator invocation recomputes the one challenge; this is an authorization action, not an autonomous retry path |
| Dedicated journal full/failing | Not by itself, unless health evidence also cannot persist | Logs rotate/drop; Capsule evidence remains authoritative | 512 MiB target inside hard 576 MiB project quota; generated rate limit and quota prevent crash/retry spam growth |
| Process crash or host restart | Startup gate closed | Three-voter quorum load, arena validation, stale-voter repair, and venue reconciliation run before any risk increase | Generated `Restart=always`, constant TOML delay, `StartLimitIntervalSec=0`, and bounded journal rate retry forever; host boot enables the unit; state cardinality unchanged |

## Verification Program

### Local non-compile gates

Each PR runs formatting, config/static validation, source-fence-static, workflow
lint, dependency policy, targeted text checks, and artifact consistency locally.
Compile-heavy Rust verification remains remote-first under repository policy.

### Required behavioral and property evidence

The publishable head must include all of the following:

- A model/state-machine test whose transition invariant proves record and state
  cardinality never exceed the fixed vector for any sequence length.
- Large concrete A→B→A tests, including at least 1,000,000 oscillations and changes
  to every formerly volatile outer-key field, ending at the same cardinality as the
  first complete cycle.
- Maximum-value construction for every Capsule section and evidence variant, exact
  file-length inspection, and `limit-1/limit/limit+1` tests. Assert the
  892,928-byte encoded maximum, 155,648-byte zero reserve, two 33,562,624-byte
  full-replica crash peaks, 16,384-byte witness, and 67,141,632-byte runtime set.
- Exhaustive direct-parent and field-join closure for every adjacent Capsule phase
  at the 10/20/10/13 maxima, including durable empty/reuse barriers for risk,
  evidence, archive, retry, lifecycle, retirement, and diagnostic wraparound.
- Quorum state-machine tests crash every A/B/witness publication order, prove that
  any two-vote current digest includes a full replica, and prove no two digests can
  both hold quorum. Non-risk-increasing work must continue through A+B, A+witness,
  and B+witness only when W's checksum-valid selector names the fully synced exact
  digest/parent/device-id record. Missing/corrupt selectors abstain, unselected
  records are never inferred, one full+invalid W is no quorum, and A+B repairs W.
  Every entry/risk-increasing send remains absent until all three votes and both
  arena replicas are byte-identical.
- Sequential voter-failure tests run arbitrarily many degraded commits, then return
  a replica stale by an arbitrary lineage distance. Assert direct automatic repair,
  no accumulated history, no stale-replica effect, no action without a current
  full-payload quorum, and entry reopening only after all-three agreement.
- Concurrent candidate tests proving the fixed request and complete capacity vector
  commit as `EntryPreparedNotAuthorized` before authorization and cannot
  oversubscribe. Crash at A+B-before-W, every W repair point, authorize/abort
  publication, and dispatch. Prove repair never authorizes/sends, the separate
  same-snapshot recheck covers trusted time, market/expiry, feeds, health, and
  capacity, stale/expired candidates all-three abort without send, and release
  follows only the durable terminal.
- Generated closed-form checks for the per-risk vector and fixed remainder: ten
  risks must sum to at most 512 MiB arena, the typed 512 MiB ballast claims, exactly
  128 protected async owners (`10*6 + 16*2 + 8*3 + 12 = 128`), separate async-task
  partitions, hard native-thread 127/128/129 boundaries backed by exactly 128
  one-MiB stacks, 512 FDs, 14 Polymarket
  wire asset ids under the 64-member global subscription cap, and generated
  `NetworkFootprint` caps/populations: HTTP owners 18/17, WS owners 16/11, origins
  19/18, DNS/TLS 18/17, protected/ordinary HTTP 12/4, DNS sockets two, physical/
  protected sockets 34/30, one WS physical per owner, and the exact 30-socket
  ballast claim. Assert the FD partition arithmetic
  `80+136+64+32+64+48+88=512`. Test 3,072 protected
  items in each NT risk/execution queue, 64 protected ingress frames, and 64 retry
  owners. Test every `limit-1/limit/limit+1` partition independently.
- Per-market admission tests proving ordinals `0..9` are never replenished, the
  eleventh candidate blocks, rediscovery at/below the contiguous retirement
  frontier cannot recreate novelty, and S3 remains exactly at most 12 objects/960
  records per market under arbitrary state oscillation.
- Full ordinary-partition, memory, task, FD, queue, disk, and archive saturation
  followed by every exit, reconciliation, settlement, and terminal transition for
  ten risks.
- Arena-unavailable tests that fill the global prepared frame, commit all 640
  per-risk receipts across ten simultaneous closures, fail each arena replica,
  restart from every full-replica+witness quorum, repair the mirror, and materialize
  each canonical state once without reevaluating volatile input.
- Source-fenced provider proof for the exclusive Bolt account, exact current-
  unresolved capture caps, and prepare/query/replay. Inject 20 and 21 open orders,
  10 and 11 active positions/redeemable claims, any continuation, inconsistent
  snapshots, concurrent external writes, partial/terminal attempts, and response
  loss. Persist the finalized Polygon block before every dispatch. Prove the exact
  terminal chain from signed hash through the exact
  `ProviderTerminalCertificate`, Polygon V2 `getOrderStatus`, a complete sorted
  unique at-most-64 set of 32-byte transaction hashes, sequential at-most-
  2,097,152-byte/4,096-log receipts, indexed `OrderFilled` logs, and exact
  post-state.
  Decode the frozen provider-table POST success envelope and its statuses (`live`,
  `matched`, `delayed`, `unmatched`) separately from GET-order statuses (`ORDER_STATUS_LIVE`,
  `ORDER_STATUS_INVALID`, `ORDER_STATUS_CANCELED_MARKET_RESOLVED`,
  `ORDER_STATUS_CANCELED`, `ORDER_STATUS_MATCHED`), and reject unknown values.
  Transport failure, non-2xx, `success=false`, or a malformed/extra-field success
  envelope must collapse to the fixed `PostDiagnosticFailure` metadata and remain
  nonterminal. Compile/source-fence signed order bytes, signatures, authorization
  headers, all SSM credential values, and every raw success/error/request buffer
  out of journal/evidence/report/alert traits; sentinel captured-sink fixtures for
  success, failure, malformed, oversize, and response loss must contain only the
  allowed redacted ids, lengths, classes, and digests and no sentinel substring.
  Exercise every trade status (`MATCHED`, `MINED`, `CONFIRMED`, `RETRYING`,
  `FAILED`) separately and prove no ordinary status is no-effect authority. The
  route fixture must resolve the current POST-schema/lifecycle-document drift over
  `unmatched`; drift or an additional value keeps activation blocked. Test
  submit-before-tombstone, tombstone-before-submit, delayed/retry,
  duplicate POST, preapproval, provider restart/rollback, response loss, and
  transaction-hash `63/64/65`, receipt-byte, and log-item boundaries. A tombstone
  acknowledgement must survive all of them and a fill certificate must expose
  every transaction hash within a provider-guaranteed cap; truncation fails the
  capability gate. No unfiltered account
  history or uptime-sized log query exists. The current V2 negative fixture must
  prove that its missing permanent tombstone keeps autonomous entry mechanically
  disabled; a timeout, FOK wording, unsigned expiration, cancel response, 404, or
  quiet chain must never satisfy the gate. Provider quantities round-trip as exact decimal/scaled
  integers; every positive dust value and raw redeemable/non-redeemable position is
  returned, with static/runtime proof of no `f64`, epsilon, or dust filter.
  `EntryPreparedNotAuthorized` proves no call could start and may only freshly
  make the final conservative commit or durably abort. Only
  `DispatchMayHaveStarted` may use the exact terminal-certificate path; every
  negative/incomplete result remains query-only and cannot authorize replay or
  release. Risk-increasing replay after that final commit is forbidden.
  Failure of a required capability blocks `AO-NT.a`, `AO-NT.b`, profile build, and
  final authorization.
- `AO-REDEEM` provider-boundary tests source-fence target, collateral, ABI, output
  asset, wallet type, SAFE address, and relayer route from resolved TOML plus the
  provider manifest; missing/mismatch blocks and no address is hardcoded. Standard
  and negative-risk fixtures call the current adapter's inherited external
  `redeemPositions(address,bytes32,condition,uint256[])` ABI with exact manifest-fixed
  dummy values and prove source uses none of them except condition. Verify the
  standard CTF path and negative-risk internal current-balance/legacy-two-argument
  path, including USDC.e-to-pUSD/PMCT output; assert Bolt never targets the internal
  ABI. Race the exact pre-balance snapshot and exclusive condition lease before
  send. Crash every exact signed-
  account-global nonce-lane acquisition, original body preparation/send, relayer-id
  query, same-nonce fence preparation/send, receipt/log, post-state, winner commit,
  and lane empty barrier. Race two conditions for one nonce and assert only one
  owner. Retry must preserve
  `(chain,wallet type,SAFE address,SAFE nonce,target,calldata hash)` and identical
  body bytes. `NEW`, `EXECUTED`, `MINED`, `FAILED`, and `INVALID` never release by
  themselves. Race original versus the manifest-bound zero-value Safe `nonce()`
  fence in both orders: redemption wins only with finalized adapter/post-state;
  fence wins only with finalized nonce advance and unchanged claim/post-balance.
  Inject an unrelated nonce consumer and require integrity halt. Prove the relayer
  accepts the explicit competing same nonce or keeps the profile disabled. Construct
  both 4,096-byte bodies and every remaining field at maximum inside the 16,384-byte
  lane. SSM is the sole grouped credential source;
  existing settlement bookkeeping cannot satisfy any redemption assertion.
- Crash injection for every row of this contract, with filesystem artifact and
  remote-effect assertions after restart.
- Restart with an open entry order, managed position, exit pending/unknown,
  settlement pending/unknown, every materialized partition at capacity, and each
  combination with S3 absent and with A, B, or witness failed/stale.
- Destroy both full media/all four payload slots and prove ordinary recovery admits
  no aggregate and initiates no effect. Then exercise the exceptional
  `CatastrophicBootstrapCertificate` with exact capped order/position/redeemable
  aggregates and source/finality, resolved-config, replacement-device, and
  exclusive-account-fence digests. Crash before and after A, B, and witness; mutate
  the aggregate/finality/fence at every boundary and prove partial certificates are
  invalidated, stale lineage cannot vote, and no S3 fact participates. After a
  complete certificate, assert no ordinal, association, terminal history, entry, or
  canonical evidence exists. Only reduction to zero and redemption of the exact
  unattributed aggregates may run, and every restart remains
  `HaltedUnknownIntegrity` until a separately authenticated repair completes.
- Separately crash `UnknownIntegrityEvidenceFence` for recoverable corrupt history
  at every publication boundary and serial wrap; assert it emits no canonical
  evidence for the fenced families, saturates all affected risk/market states and
  trusted current+next windows plus lost/current system states, and never
  clears/unsaturates. This fence must not be confused with or used to clear a
  catastrophic sentinel root.
- S3 unavailable, slow, duplicate, partial-response, lost-response, checksum
  conflict, retention deletion failure, policy drift, and return-to-health cases.
- S3 tests against a dedicated bucket whose version status is absent; `Enabled` and
  `Suspended` fail. Verify all 365 market-ring slot phases, delete/list/empty before
  reuse, day-tag/index wrap after arbitrary downtime, one in-flight object targeting
  an existing fixed key (no `+1` key), the
  prune-protected 258-object/2,163,350,912-byte legacy slot with upload longer than
  365 days and full remote revalidation, and exact global maxima of 1,261,698
  objects and 3,314,119,482,752 bytes. Exercise the exact registry
  `Unverified -> EmptyVerified -> Owned -> ReuseBlocked -> DeletePrepared ->
  Deleting -> VerifyingEmpty -> EmptyVerified -> Owned`, including the initial
  366-prefix empty census, numeric cursor values `0..3456` for market keys and `0..258` for legacy
  keys (`0..257` are key indices and `258` is terminal),
  at-most-64-key deletes, HEAD-per-key verification, the final `MaxKeys=1` list,
  its 262,144-byte response cap, unexpected keys, and prove no opaque continuation
  token is persisted. Continuation metadata must fit inside the 4,096-byte envelope
  and never enlarge the 8,388,608-byte payload. Lifecycle delay has no role in the
  proof. Run generated `crash_s3_cohort_001..008`,
  `crash_s3_retry_001..003`, and `crash_s3_legacy_001`; the legacy terminal must
  pass `EmptyVerified -> Retired` exactly once and never reenter ownership.
- Automatic drain and entry-resume after S3 and every other dependency returns.
- Repeated market rollover at least 100,000 times with disconnections at every
  phase; assert 14 durable bundles own at most one Polymarket asset id each, shared
  by book/trade consumers, under the unchanged 64-member global cap. Assert no
  expired subscribe request and exactly zero per-asset unsubscribe calls. Assert
  stable lease/cache/task/FD/retry/generation cardinality, bounded error count/rate,
  and no leaked connection, task, file, client, or retry episode. Exercise
  a deliberately nonresponsive provider read at every rollover edge; prove the
  TOML join deadline self-terminates and restart creates only the exact durable set.
  Exercise
  `Absent -> Requested` only after
  transport write and `Requested -> Observed` only after the first valid
  current-generation asset-specific full book snapshot or source-fenced sequence-
  complete baseline. Inject delta and trade messages before the baseline and prove
  they only invalidate/resnapshot; send success is never an
  ACK, entry requires `Observed` plus a fresh complete book for every target, and any timeout
  closes/joins and replaces the exact-set generation. Include restart after expiry,
  exact Gamma `slug=<TOML-template+lane+trusted-window>&limit=2&offset=0` with bounded
  body/items, zero/one/two/cap-overflow/wrong-identity results, missing token ids and
  windows, multiple-identity churn, constant-work downtime rebase after arbitrary/
  multi-wrap gaps, and adjacent `MAX->0`. Missing token ids must remain one
  `DiscoveryHydrating` retry with no episode. Accept and durably bind only the first
  fully hydrated `GammaMarketBinding` with exact tuple
  `(gamma_market_id, condition_id, question_id, exact_slug, trusted window open/close,
  neg_risk_mode, ordered exactly-two [(outcome_index, normalized_outcome,
  clob_token_id)])`; every later field/order/token mutation blocks rather than
  rebinding. Derive a separate `EvidenceEpisodeId` only from stable logical
  strategy/target/venue, non-temporal market/condition/question ids, ordered
  outcome/token binding, and applicable risk ordinal. Oscillate slug, open/close,
  timestamp, and every transient field indefinitely and prove novelty/cardinality
  do not reset; only a genuinely new market/condition id rolls the episode. Then
  commit zero-after-close as `ClosedWindowNoAcceptedCandidate` without new risk,
  episode, evidence, or a historical-absence claim, keep account/risk reconciliation
  authoritative, and block all ambiguity. Assert one 30-second freshness lease covers at most two
  current+next exact-slug queries plus 20 order/10 risk/10 settlement queries,
  rejection when trusted samples cross the pair or lease, fixed query/transition
  count, every member `Absent` in each new generation/process, authoritative REST
  order ownership, and at most one episode per lane/serial.
- Oversize WebSocket, HTTP, evidence, legacy, Capsule, and archive inputs fail
  before allocation/publication and do not weaken existing-risk behavior.
- Migration fixtures for the exact incident artifact shape, each 2-MiB recovery-file
  boundary, the TOML `N=1,048,576` legacy-record cap and overflow, the
  2,151,809,024-byte total sealed-input ceiling,
  torn tail, malformed middle, conflicting facts, venue/SSM/S3 outage, old-release
  compatibility fence, and crashes at every migration row. Production invocation
  remains mechanically impossible through `AO-MIGRATION`; `AO-INTEGRATION` may
  remove/source-fence the last legacy code path and expose only one disabled
  stopped-service entrypoint. No PR, CI job, deploy helper, or test invokes the
  production cutover, and the later operator-run path proves writable legacy and
  Capsule authorities never coexist.
- Prove the old runtime disables entry but continues exit/settlement until two
  matching finalized captures show zero orders, positions/dust, claims, and
  settlements. Inject every dependency failure and nonzero fact before stop; the
  old runtime must remain active and migration must not mutate authority.
- Migration adversaries include the old/same-UID/alternate service attempting
  opens before/after census, writable inherited FD, closed-FD writable
  `MAP_SHARED`, outside-root hardlink, directory-topology drift,
  ACL/group/capability/mount bypass, warm
  2-GiB payload cache, hash mutation, and path exchange. Require single-link clean
  immutable inodes/parents, exclusive migrator read ACL, fadvise/mincore zero
  payload residency, and the generated metadata-memory cap before publication.
  Crash every bootstrap slot on both full
  replicas, both mirrored arenas, every witness record/selector, initial quorum,
  `MIG-FENCE-001..006`, `ACTIVATE-001..003` against hermetic selector roots, path
  state, and archive-lock state
  `MigratorHeld -> TransferPrepared -> RuntimeHeld`. Verify every crash leaves one
  resumable exclusive writer and uploader/runtime activation occurs only after the
  handoff. Imported ids are legacy-only with seeded novelty and cannot recur into
  the arena. Assert a deterministic 16,384-descriptor inventory containing exactly
  one preallocated blocker and at most 16,383 source paths; crash create/sync,
  exchange/parent-sync/publication-guard, and quota-reduction boundaries and accept
  only the two exact inode/name mappings. Assert an at-most-258 used object prefix whose
  payload is at most 8,388,608 bytes plus exactly one 4,096-byte envelope;
  2,163,350,912 total legacy bytes; remote revalidation before
  `DeletionAuthorized`; no source deletion before authorization; and no remote
  deletion before quorum-durable `LocalEgressDeleted` plus 365-day expiry; fixed
  quarantine remains present and exact through both deletion phases.
- Repeat legacy input in every permitted record/file ordering and after crashes.
  #883 exact-byte-egress-allowlisted raw history archives unchanged and is never
  semantically imported. Registered JSONL at most 2,097,152 bytes becomes both a
  semantic import and a classified binary frame of exactly its source length;
  prove no raw sensitive field survives and a non-fitting encoding blocks. Every
  unapproved raw family is permanent quarantine, never uploaded or deleted.
  Stream one sealed path at a time with one fixed aligned 33,554,432-byte direct-I/O
  input buffer owned only by the Ordinary workspace. Verify `STATX_DIOALIGN`, aligned
  sealed-tail reads, mandatory `O_DIRECT`/`RWF_DIRECT`, and no buffered fallback;
  fadvise/mincore must prove zero source payload-data cache, while allocated
  directory blocks plus inode/dentry/xattr rows fit `M_legacy_meta=134,217,728`.
  Prove no dirty/writeback local cache, scratch, or merge pass exists. Bound
  inventory by `F_total=16,384` 64-byte virtual-range/metadata-index descriptors
  (`1,048,576` bytes), source reopen/egress metadata with complete <=512-B paths by
  `F*640=10,485,760`
  bytes, and semantic state by `N=1,048,576` 40-byte descriptors (`41,943,040`
  bytes). Binary-search virtual ranges and reopen with at most four root FDs plus
  one data FD; every equal-digest descriptor is reread once with one <=512-B
  reference key. Assert `F_source=16,383`, `N+2F_source=1,081,342` opens and
  `3S+4A*F_source+2AN=15,313,780,736` aligned bytes. Assert
  `S=2,151,809,024`, `S_egress<=S`,
  `F_egress<=F_source`, `L_actual=S_egress+640F_egress<=2,162,294,144`,
  `258*8,388,608=2,164,260,864` payload capacity, and
  `L_actual+object_count*4,096<=2,163,350,912` stored bytes. Continuation metadata stays inside
  the envelope and cannot change the payload bound. Saturate the exact
  134,217,728-byte workspace: 33,554,432 direct-I/O input buffer + 41,943,040 semantic
  descriptors + 1,048,576 inventory + 8,392,704 object buffer + 33,554,432
  Feather/decoder + 10,485,760 source reopen/egress metadata + 5,238,784 join/key/slack. HMAC rotation/removal cannot change
  frozen legacy bytes.
  Separately, active-recovery component ordering must produce byte-identical risk
  ordinals/Capsule; conflict or an eleventh component blocks without partial
  authority.
- Physical resource checks saturate the 2,147,483,648-byte ordinary pool,
  536,870,912-byte recovery arena, exact 536,870,912-byte ballast equality, and
  536,870,912-byte non-pool allowance under the 3,758,096,384-byte operational and
  4,026,531,840-byte hard main limits. Substitute/measure/retouch every stack,
  socket claim with swap disabled: 134,217,728 bytes of resident stacks,
  188,743,680 bytes across `30*6,291,456` protected sockets, and 213,909,504
  permanently locked bytes must always sum with touched free ballast to the exact
  claim. Recovery/config page cache is owned only by `N_main`, never ballast.
  Retain the 6,291,456-byte per-socket charge only
  if effective kernel buffers, TLS/user buffers, and every kernel-object summand are
  enumerated with no opaque slab. Thirty protected sockets reserve full `C` from
  ballast and four ordinary sockets reserve full `C` from Ordinary before open. On
  close, main-cgroup residue moves to `N_main.net_retained` and only
  root/unmanaged residue moves to `K_host` before `C` is retouched. Generate
  `N_main = native-thread guard/VMA/page-table metadata (resident stack pages excluded) + ELF PT_LOAD + pinned DSO PT_LOAD +
  loader/vDSO/static TLS/mappings + VMAs/page-table bound from declared virtual
  mappings + fixed-arena allocator metadata + recovery/config page cache + declared
  runtime/control objects + process-attributed nonsocket kernel objects +
  main-cgroup retained socket rows` and
  `K_host = signed-AMI pinned base/kernel static + ceil(memtotal_max/base_page)*BTF
  struct-page bytes + perCPU + per-device + global network/fs/cgroup state +
  route/neighbour and DNS UDP/TLS caches + uncharged-only journal/filesystem cache +
  root/unmanaged retained socket states`. Prove disjoint ownership and derive every coefficient and the
  536,870,912-byte non-pool allowance from resolved TOML, build/kernel manifest, and
  signed AMI manifest; missing terms block and runtime observations are drift checks
  only. Then cross the 256-MiB guard, kill/recharge the 1-GiB host reserve,
  protect the 671,088,640-byte kernel gap, reject `MemTotal` outside
  `[8,053,063,680,8,589,934,592]`, exercise all 64
  protected/32 ordinary retry owners, and force journal rotation overlap against
  the hard 576-MiB project quota.
- Archive-worker tests complete the maximum 8,392,704-byte object upload, maximum
  delete/HEAD/final-list response, and IMDS operation within 201,326,592 bytes,
  then cross the 268,435,456-byte hard boundary and prove local evidence remains.
  Assert a 32-permit async semaphore distinct from OS `TasksMax=16`, 64 FDs, two
  exact origins, one sequential live/zero-idle HTTP/1.1 connection, and no redirect,
  proxy, SDK, or library retry.
- Per-device disk proofs assign project ids/inheritance on every actual recovery,
  witness, report, journal, release, legacy, and system-mutable filesystem across
  exactly seven project classes and
  mechanically check both future-byte and future-inode floor equations. Require
  three distinct recovery device ids; test shared/separate devices, missing
  `prjquota`, read-only-root/path drift, both 256-MiB/16,384-inode tmpfs mounts,
  sparse allocation, xattrs, directories, and exact inode maxima of 16 per full
  replica, 8 witness, 4 reports, 256 journal, 49,152 releases, 16,384 legacy, and
  65,536 system-mutable. Assert migration performs no local sort spill or scratch
  allocation. Assert
  `byte_floor(d)=max(applicable class floors)`, never a sum: 10 GiB if any
  recovery/data class applies, otherwise 2 GiB for root/log; assert
  `inode_floor(d)=65,536` exactly once per device, and
  10,816,061,440 B/65,552 inodes per recovery device,
  13,438,550,016 B/81,924 migration-data, 10,754,195,456 B/65,540 post-data,
  5,435,883,520 B/180,488 root/log+witness, legacy 2,684,354,560 B/16,384,
  and post/migration-or-quarantine host peaks
  3,462,463,488/6,146,818,048 bytes.
- AWS/network source fences construct only IMDSv2 credentials: one generation,
  timer, 65,536-byte response, and in-flight request in each process, with all
  environment/shared-file/ECS/web-identity/process/default-chain paths disabled.
  Generate immutable `NetworkFootprint` from resolved TOML and require every client,
  connect, spawn, and raw socket to consume declared HTTP-owner, WS-owner, origin,
  DNS/TLS, and protection rows before open; unknown paths block. Prove exact
  cap/population pairs HTTP 18/17, WS 16/11, origins 19/18, and DNS/TLS 18/17, plus
  12 protected/four ordinary HTTP, two DNS sockets, 34 physical/30 protected
  sockets, and `80+136+64+32+64+48+88=512` protected FDs. Generate and inspect
  `NetworkLifetimeFootprint`: autotune is disabled and verified; effective
  `SO_RCVBUF`/`SO_SNDBUF`, TLS/user buffers, kernel-object multiplicities,
  route/neighbour entries and DNS UDP/TLS caches, global
  protected/ordinary dial token buckets, per-owner minimum reconnect/stable reset,
  pinned `TIME_WAIT`, FIN/orphan, and conntrack retention/cardinality, and
  ephemeral-port bounds are exact. Assert
  `retained <= concurrency*(ceil(H/min_dial)+1)`. Full worst-case ballast is
  acquired before open; closed residues retain a disjoint
  `N_main.net_retained` claim while cgroup-charged and only root/unmanaged rows
  transfer to `K_host` before live charge is reused. Exercise close storms against
  `memory.current`, `memory.stat sock`, and `memory.events` and prove no
  retouch/redial crosses the main operational or hard limit. Exercise every cap at
  `limit-1/limit/limit+1`, HTTP/1.1 serial
  dialing with zero idle/redirect/proxy/library retry, close/join-before-WS-redial,
  and archive two-origin/one-live state. Observations cannot supply a bound.
- Verify host-wide `/proc/swaps` is empty, no swap/zram unit exists, every generated
  unit/slice has `MemorySwapMax=0`, and a swap activation fails readiness. Inspect
  the closed enabled-unit/socket/timer census, all effective `TasksMax` and
  `LimitNOFILE` values, `kernel.pid_max=512`, and `fs.file-max=8192`; exercise each
  unit/slice and host cap at limit and cap-plus-one. Enumerate the at-most-16
  non-Bolt network owners/64 sockets and run their combined close/retry storm
  through the same host lifetime projection. Any unknown unit, socket activation,
  timer, owner, or retry fails startup.
- Verify effective `LimitSTACK=1,048,576`, exact-size builders, and provider source
  fences. At 127/128/129 native threads, inspect every
  `/proc/<pid>/task/<tid>/maps` stack mapping and exercise deep-stack faults; no
  mapping or growth may exceed its preclaimed resident/guard rows.
- Source-fence proof for all provider runtime bytes/metadata, all spawns, all file
  and socket acquisition, and the absence of legacy/S3 recovery reads. Subscription
  source fences permit autonomous-profile calls only through the sole supervisor;
  prove one Polymarket asset id per bundle shared by book/trade, no claimed server
  ACK, autonomous-profile source-fence rejection of every per-asset unsubscribe,
  whole-generation close/join/replacement as the sole lifecycle path, the
  14/64 caps, and REST order authority. Any probe exemption is explicitly enumerated
  and unavailable to strategy/runtime code.
- Source census proves every JSONL consumer uses the bounded Rust export, the
  unbounded report reader and Python migrator are absent, and production Capsule
  activation cannot occur before #763, rollover, host, and `AO-MIGRATION` are
  present. `AO-INTEGRATION` may remove/source-fence the final legacy path and wire
  only the disabled registered entrypoint; no PR, CI, deploy helper, or verification
  command invokes migration or profile enablement.
- Generated/effective-unit tests run thousands of crash loops and a host reboot with
  `Restart=always`, the TOML restart delay, `StartLimitIntervalSec=0`, journal rate
  limiting, and no operator reset. Alert tests hold the transport unavailable while
  every fixed health state oscillates, then prove one latest-state message per slot
  resumes automatically with no queue/history growth.

Tests may use a large finite iteration witness, but the unbounded claim comes from a checked
transition invariant and fixed schema/cardinality—not elapsed soak time.

### Exact-head remote gates

For each issue-bound PR:

1. Start from fresh then-current `origin/main` and keep the branch limited to the
   declared slice.
2. Run local non-compile gates and resolve all findings.
3. Commit and publish with `just sandbox-safe-push`.
4. Open/update a draft PR and run exact-head remote Rust verification.
5. Integrate only after exact-head required checks and review for that slice.

The final integration PR must be ready, not draft, and pass exact-head root CI,
backtester CI, provider source fences, all autonomous property/crash tests, and the
repository's required native review. The primary orchestration session performs an
internal adversarial audit of the publishable SHA using the reviewer source and
model selected by `ci/ai-review.toml`; the immutable receipt records the exact
configured source and model without embedding either value in workflow or prompt
text. An independent external review is requested only after that SHA is green and
all local findings are closed.
All valid comments are resolved at a new exact head. Merge uses repository
governance and merge queue; agents do not merge manually.

### Authorization gate

Passing tests and merging code do not authorize production. The final report lists
implemented evidence, exact SHAs, remote checks, review decisions, and every
unmeasured claim, then issues one binary ruling:

```text
AUTONOMOUS OPERATION: AUTHORIZED
```

or

```text
AUTONOMOUS OPERATION: BLOCKED — <specific unmet invariant>
```

Even an `AUTHORIZED` engineering ruling requires separate operator approval for a
supervised live canary, EC2 start, deployment, or live trade.
