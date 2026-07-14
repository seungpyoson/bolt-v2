# Continuous Operation Migration and PR Contract

This document defines the one-way data cutover and the independently reviewable
delivery graph. Symbolic labels `AO-*` are stable issue contracts whose exact
scope, dependency edges, and safe intermediate states are frozen below. Creating
or amending GitHub issues is an external mutation and requires separate owner
confirmation. After Claude approves this package, but before any implementation
branch starts, every symbolic node must be mapped to a real issue number and every
relation made bidirectional without changing this graph. Existing issues #1354,
#883, and #763 require the amendments below because their current contracts are
weaker or conflict with the selected architecture.

Implementation is still prohibited until this finalized package receives Claude
`APPROVE` and the issue bindings exist.

## Existing Durable Inputs

The migration inventory is explicit:

| Current input | Current role | Autonomous treatment |
|---|---|---|
| `persistence.decision_evidence.order_intents_relative_path` (`bolt-v3/decision-evidence/order-intents.jsonl` in the tracked config) | Mixed audit plus submit-reservation, fill, order, position, and settlement recovery | Stream at ≤2,097,152 bytes; derive Capsule recovery and a length-preserving classified binary history frame with no raw sensitive field; install an old-binary fence; never read it for recovery after publication |
| `risk.kill_switch.state_path` (`state/kill-switch.json`) | Bounded kill-switch authority | Import at ≤65,536 bytes into the Capsule control/risk section; remove the runtime file store for autonomous mode |
| Kill-switch sibling manual-recovery audit | Unbounded append-only operator history | Accept only within its 2,097,152-byte migration ceiling; emit length-preserving classified binary history; import the current effective halt only from the validated state store, not from last audit text |
| Optional `risk.basket_execution.state_path` | Bounded complete-set workflow state when configured | Autonomous BTC edge-taker profile rejects this incompatible strategy block; a future autonomous basket profile must add a reviewed Capsule adapter. If a legacy file exists, inventory it at ≤65,536 bytes and retain/archive it as history, never infer active BTC risk from it |
| NT saved state/cache/catalog | Derived NT projection | Autonomous profile sets load/save false and never treats it as authority; every inode and allocated block is charged to the same sealed legacy project. Only an exact-byte family approved by the #883 closed registry may enter bounded archive/deletion; every other family remains permanently bounded local quarantine and is not a deletion candidate |
| `catalog/live/<instance-id>/...` Feather and seven JSONL capture families | Raw diagnostic/runtime capture | Incident evidence only. A raw-only family explicitly approved for exact-byte egress by the #883 closed registry is validated and archived length-preserving; it is never semantically sorted or imported. An unregistered or unapproved family is quarantined whole and is never uploaded. Only registered JSONL recovery/evidence frames at most 2 MiB enter semantic classification. Autonomous capture is disabled |
| Journald and deployed release directories | Operations, not recovery truth | Dedicated journal rotates; deploy keeps two releases and one bounded staging release |

The four recovery inputs have an aggregate 4,325,376-byte ceiling and raw/capture
payload has a 2,147,483,648-byte ceiling, for
`S=2,151,809,024` logical source bytes. `S_egress` is the disjoint sum of registered
JSONL source lengths (whose classified binary replacements have exactly the same
length) and exact-byte-egress-allowlisted raw-only lengths. `S_egress<=S`; all
unapproved bytes form `S_quarantine=S-S_egress` and never leave the host.
All raw/capture/catalog/NT state/cache/recovery files, directories, allocation
rounding, and xattrs share a hard 2,684,354,560-byte allocated-block ceiling and
16,384 total-inode ceiling across at most four configured filesystems. After the
kernel fence, migration requires source usage at most 2,684,350,464 bytes and 16,383
inodes, creates and parent-syncs one same-filesystem 4,096-byte/one-inode blocking
directory, and only then assigns project ids/inheritance and verifies the complete
2,684,354,560-byte/16,384-inode vector with `statx.stx_blocks*512` and quota
`curspace`. The blocker is never created after quota sealing. Preflight refuses any limit or missing
`prjquota`; it never truncates or invents an overflow path.

The migrator writes no sort scratch, run file, descriptor file, or local archive
staging file. It streams one sealed path at a time with one fixed aligned
33,554,432-byte direct-I/O input buffer in the Ordinary workspace. Every source
read/reread uses `O_DIRECT`/`RWF_DIRECT`; preflight verifies `STATX_DIOALIGN`,
aligned sealed-tail handling, and no buffered fallback. After the sealed clean/
unmapped/single-link fence, `POSIX_FADV_DONTNEED` plus bounded-window `mincore`
proves source payload-data cache zero. Directory/inode/dentry/xattr state is
separately bounded, never called zero. It builds the
outgoing object only in its fixed memory buffer. Its
in-memory maxima are `F_total=16,384` path descriptors at 64 bytes each, including
the one blocker, `F_total*640` source reopen/egress metadata bytes, at most
`F_source=16,383` source-data paths, and `N=1,048,576` semantic descriptors at 40
bytes each.

The target authority is one logical Capsule graph on two full recovery replicas
plus a digest-only witness on a third device. Each full replica contains two
1,048,576-byte payload slots, two 4,096-byte manifest allocations including the
manifest temporary, and one 31,457,280-byte arena. The witness contains two
4,096-byte records, one 4,096-byte selector, and one 4,096-byte selector temporary.
The exact runtime recovery set is 67,141,632 bytes; retained recovery inputs make
the cutover peak 71,467,008 bytes. The configured project ceilings are 78,643,200
bytes for each full replica and 65,536 bytes for the witness, or 157,351,936 bytes
aggregate. The two replicas and witness must resolve to three different device ids.
A witness record counts as a vote only when the checksum-valid selector names that
fully synced record and its digest, parent digest, and configured witness device id
all match. A missing/corrupt selector abstains; an unselected record is never
inferred. A+B may repair W, while one full replica plus invalid W is no quorum.

## One-Way Migration State Machine

```text
LegacyAuthority
  -> EntryDisabledAndAuthoritativelyFlat
  -> QuiescedAndLocked
  -> Inventoried
  -> ParsedOrQuarantined
  -> VenueReconciled
  -> ArchiveAndSemanticSetFrozen
  -> ReplicaPairsAndWitnessStaged
  -> OldBinaryFenced
  -> ThreeVoterBootstrapPublished
  -> ArchiveLockTransferPrepared
  -> ArchiveLockRuntimeHeld
  -> LegacyHistoryPending
  -> LegacyHistoryArchived | BoundedQuarantine
```

The same fixed AO-HOST current/staging selector pair owns the separately registered
production mode sequence; this package does not invoke it:

```text
Legacy --ACTIVATE-001--> Migration
Migration --ACTIVATE-002--> CapsuleDisabled
CapsuleDisabled --ACTIVATE-003--> Autonomous
```

There is no reverse or skip edge. Every edge writes and syncs the inactive fixed
record under the sole parent-directory selector lock, compares current/inactive-
before before mutation, validates current/prepared-target immediately before
exchange, exchanges the pair, syncs the parent, and validates the
exact pre/post inode/digest mapping. `ACTIVATE-001` requires explicit operator
authorization plus the fresh flat certificate and atomically selects the exact
reviewed integration release/admission record together with `Migration` mode.
`ACTIVATE-002` requires selected A+B+witness, identical arenas, and `RuntimeHeld`
while retaining that exact release. `ACTIVATE-003` retains the exact release/device
epoch and requires the exact green provider, resource, source-fence, review, and
engineering-authorization manifest plus new operator approval bound to the whole
candidate. A crash resumes the selected mode and never infers the later one.
The operator-controlled signer is outside Bolt/SSM/the host; runtime contains only
the immutable verification key and can never create an approval. Before any stop or
mutation, the exact 256-byte challenge in the 4,096-byte Ordinary union workspace is signed while current risk management
continues; timeout/crash/staleness changes no authority. Every authorization envelope signs the transition id, selected-current and
inactive-before digests, both inode/device identities, complete target core digest, and
ordered prerequisite-evidence root. It is unusable for any other edge, state,
mapping, target, or evidence set.

### 1. Compatibility preflight, quiescence, lock, and inventory

A read-only compatibility probe first loads the typed TOML, computes maximum Capsule
encoding, proves the 67,141,632-byte runtime and 71,467,008-byte cutover recovery
sets against the 157,351,936-byte aggregate project ceiling, proves that the
migrator has no local scratch or spill path, groups all seven project classes
across at most four devices, and proves each device's
`f_bavail(d) - Σremaining_bytes(i) >= byte_floor(d)` and
`f_favail(d) - Σremaining_inodes(i) >= inode_floor(d)`. Here
`byte_floor(d)` is the maximum applicable class floor, never their sum: 10 GiB when
any recovery/data class applies, otherwise 2 GiB for root/log; `inode_floor(d)` is
65,536 exactly once per device. The exact
cold minima are 10,816,061,440 bytes/65,552 inodes for each recovery device,
13,438,550,016 bytes/81,924 inodes for a data device during migration, and
5,435,883,520 bytes/180,488 inodes for the root/log device holding the witness.
Colocation sums all applicable claims but applies only the maximum byte floor and
one inode floor; it does not substitute a global total. Preflight also proves all
staging paths required by the exchange share
a filesystem and proves Linux `renameat2(RENAME_EXCHANGE)` support. It may report
incompatibility but does not inventory mutable authority or stage output.

The old runtime first durably disables entry while retaining all exit,
reconciliation, and settlement workers. Under the exclusive Bolt-account writer
fence it obtains two matching bounded venue/chain captures around the configured
finalized head and durably records zero open/unknown orders, zero positions
including dust, zero redeemable claims, and zero pending settlements. If SSM,
venue, chain, or any exact query is unavailable or nonzero, migration does not
start and the old runtime continues managing risk. The final flat certificate and
entry-disable remain valid through the systemd stop transaction; any drift aborts
before ownership changes.

Only then does the release-owned migration service run under a dedicated privileged
migration identity. It stops and conflicts/masks the old runtime unit, waits for its cgroup to
empty, then changes every legacy root to migration-identity ownership with mode
`0700`, removes every ACL/group grant, and verifies that all runtime/service
identities lack `CAP_DAC_OVERRIDE` and `CAP_FOWNER`. The generated units expose no
writable bind of those roots to another service. Every accepted regular file must
have exactly one contained link; directories instead require the generated exact
contained-parent/mount topology. Symlink, nested mount, bind alias, or a regular
file with `st_nlink!=1` blocks. The
migrator proves no writable `/proc/*/fd` and no writable `MAP_SHARED`
`/proc/*/maps` reference to any legacy inode, fsyncs files and `syncfs`, applies
immutable inode/parent protection, and restricts reads to the migrator. It issues
Rust-native `POSIX_FADV_DONTNEED` and verifies zero resident payload pages with
bounded-window `mincore`; persistent residency blocks. The continuing kernel
fence—not the scan or advisory lock—prevents a new opener, alias, or mapping between
inventory, final hash, and `RENAME_EXCHANGE`.

The service then acquires an exclusive cutover lock held from authoritative
inventory through Capsule publication and new-runtime activation. A crash restarts
the migration service while the old unit remains masked and the migration-owned
`0700` roots remain inaccessible to every runtime identity. Any ownership, mode,
ACL, capability, mount, namespace, link-count, open-FD/mapping, immutable-bit, or
payload-residency drift blocks without staging or publishing.

The separate archive-writer registry begins `MigratorHeld`. Its only handoff is
`MigratorHeld -> TransferPrepared -> RuntimeHeld`: the migrator remains the sole
writer through preparation, the transfer descriptor names the exact uploader/
runtime generation, and the new writer can activate only after the migrator is
quiescent and `RuntimeHeld` is quorum-durable. A crash in any phase deterministically
restores one owner; no phase permits two writers or leaves archive work ownerless.

Only under that fence does the Rust migrator resolve paths without following
symlinks and record size, deterministic file inventory, permissions, inode identity,
and SHA-256 with bounded buffers. It resolves required SSM values without displaying
them and queries venue orders, positions, balances, and settlement state. Failure
remains quiescent under one retry owner; it never re-enables the old runtime around a
partially staged authority.

Every provider quantity is exact decimal or scaled integer, and every positive dust
balance plus raw redeemable/non-redeemable position is retained. The migrator stores
a finalized Polygon block before any reconciliation action. For an unresolved
signed order hash it accepts only the source-fenced
`ProviderTerminalCertificate`. `Filled` must carry the complete sorted unique
at-most-64 set of canonical 32-byte transaction hashes and agree with finalized V2
status, sequential at-most-2,097,152-byte/4,096-log receipts, indexed
`OrderFilled` logs, and exact post-state. `PermanentlyTombstonedNoEffect` must be a
linearizable, restart/rollback-durable exact-hash fence ordered behind every
submit/delay/retry/match/duplicate/preapproval path and must agree with untouched
finalized V2 status and exact post-state. Absence/404, cancel/not-canceled, elapsed
time, unsigned wire expiration, and ordinary order/trade status are diagnostic and
nonterminal alone. The current V2 provider lacks this permanent tombstone and a
reviewed completeness/at-most-64 transaction-hash guarantee, so the autonomous
profile remains invalid until a pinned provider revision passes both capability
gates. The migrator never performs an unfiltered account-wide terminal-
history or uptime-sized log query and never uses `f64`/dust filtering.

### 2. Derive recovery state

The JSONL parser streams records and keeps only the fixed unresolved model. Exact
finalized venue/chain facts override stale local status; absence or an intermediate
wire status does not. Local records preserve prepared intent, client/venue mapping,
kill-switch state, capacity ownership, and causal evidence needed to avoid assuming
flat.

Every malformed/conflicting input is assigned exactly one closed class:

1. `HistoricalOnly`: the registered schema proves the frame cannot authorize,
   identify, size, or report any external action or current authority. Quarantine
   its bytes. Saturate every novelty bit for the reconstructable episode, or install
   `UnknownIntegrityEvidenceFence` when identity is unavailable. Cutover may proceed
   because the independent pre-stop certificate already proves the account flat.
2. `TerminalAssociationOnly`: every referenced external identity is exact and has
   a source-fenced permanent terminal proof, exact amount is known, the account is
   authoritatively flat, and ambiguity is limited to historical booking association,
   ordinal, or evidence state. Import no association; quarantine/saturate exactly as
   above and permit cutover.
3. `RecoveryBearingUnsafe`: the bytes can change whether an external action may have
   started, its identity/amount/account, terminality, current exposure, settlement,
   capacity, or the truth of the flat certificate; an identity is missing; a
   terminal proof is not permanent; or any current fact is nonzero/over cap. This
   blocks cutover. Venue aggregates cannot turn it into authority and no operatorless
   guess is permitted.

A trailing incomplete line uses the same classifier; it is never assumed
historical. A locally terminal but externally unresolved action is class 3. Since
migration requires zero current risk, it never invents a quarantine risk ordinal or
adopts a nonzero venue aggregate. Current-account capture still probes one beyond
the maxima--21 orders and 11 positions/redeemable claims--with no continuation; any
item, cursor, nonzero amount, or inconsistent snapshot blocks before the old runtime
is stopped.

Historical terminal component ordering is deterministic across rescans. The
migrator forms components only from exact permanently terminal external ids,
bytewise sorts `(canonical market id, key-kind rank, primary key, outcome id)`, and
uses that order solely for classified history and novelty saturation. Timestamps,
prices, source order, and diagnostics never participate. A missing/conflicting id,
nonterminal action, nonzero aggregate, or eleventh component is
`RecoveryBearingUnsafe` and blocks. The bootstrap creates no active risk ordinal:
all `10/20/10` current risk/order/settlement slots are empty by the flat certificate.

The TOML legacy record cap is `N=1,048,576`; the next record blocks cutover. The
current incident file (1,254,325 bytes, 272 records) fits both that cap and the 2 MiB
per-input cap. Those facts do not replace maximum-size and corrupt-input tests.

### 3. Freeze one deterministic imported set

Historical inputs pass the #883 closed classification registry before egress. An
exact-byte-egress-allowlisted Feather/raw family is validated and archived
unchanged; it is never semantically sorted or imported. A raw family that cannot be
approved as exact bytes is quarantined as a whole under the sealed legacy ceiling
and never enters deletion. Registered JSONL recovery/evidence frames of at most
2,097,152 bytes become semantic records and length-preserving classified binary
history frames: approved fields and pseudonyms must fit inside the original frame
length and the remainder is padding. Raw JSONL bytes never egress; failure to fit
blocks rather than truncating. Raw identifiers required for current
order/fill/settlement reconciliation remain only in bounded Capsule workflow
fields. Any current-risk pseudonym needed after cutover is computed once and stored
in the Capsule as payload, never episode identity.

For semantic records, the canonical key is exactly `(stream class, stable business
episode, canonical semantic state)`. Prices, timestamps, feed flags, retry counts,
diagnostics, raw operational identifiers, and source offsets cannot enter it. Keys
are at most 512 bytes. The migrator streams one sealed path at a time and writes no
sort scratch, merge run, descriptor file, or local object staging file:

1. Accept normalized relative paths of at most 512 bytes and build at most
   `F_total=16,384` fixed 64-byte path descriptors in lexicographic order, one of
   which is the pre-created blocker: exactly `F_total*64=1,048,576` bytes. Each stores type, length, SHA-256, virtual range, and an
   index into its 640-byte source-metadata row.
2. Build exactly one fixed 640-byte source reopen/egress metadata row per accepted
   path. It contains root index and the complete normalized relative path; at the
   maximum this is `F_total*640=10,485,760` bytes. The row also classifies the path as
   transformed JSONL egress, exact-byte raw egress, permanent quarantine, or the
   unique non-egress migration blocker.
3. For registered JSONL, parse only complete at-most-2-MiB frames and retain at most
   `N=1,048,576` in-memory 40-byte semantic descriptors containing canonical-key
   SHA-256 digest, `u32` virtual source offset, and `u32` frame length. This is exactly
   `N*40=41,943,040` bytes.
4. Sort those descriptors in memory by digest. Binary-search its virtual range,
   resolve the metadata row, and use `openat2` beneath one of at most four sealed
   root FDs plus one source-data FD. For every equal digest, retain one 512-byte
   reference key and reread each original frame once for complete-key comparison.
   Equal keys use only the
   schema-generated commutative, associative, and idempotent join; a digest collision
   naming different keys blocks cutover.
5. Freeze the semantic-set digest and the classified/exact-byte egress inventory/object
   table. Object bytes are regenerated directly from the sealed input and fixed
   metadata into the one object buffer.

The one open input path uses a fixed aligned 33,554,432-byte direct-I/O input
buffer owned only by the Ordinary workspace. `O_DIRECT`/`RWF_DIRECT` is mandatory,
preflight verifies `STATX_DIOALIGN` and aligned sealed-tail reads, and buffered
fallback is forbidden. The seal first proves clean/unmapped/single-link files,
issues `POSIX_FADV_DONTNEED`, and verifies zero resident payload pages with
bounded-window `mincore`, so source payload-data cache is zero after sealing.
Directory blocks, inodes, dentries, and xattrs have a separate generated
`M_legacy_meta=134,217,728`-byte cap, partitioned by effective charge owner into
`N_migration` or `K_host`; overflow blocks. The exact
134,217,728-byte workspace is 33,554,432 input buffer + 41,943,040 semantic descriptors + 1,048,576
path inventory + 8,392,704 object buffer (8,388,608 payload plus 4,096 envelope) +
33,554,432 Feather/decoder + 10,485,760 source reopen/egress metadata + 5,238,784 join/key/slack. There is no dirty/writeback
local cache, scratch, or merge pass.

For `A=4,096`, one clean generation performs at most
`N+2F_source=1,081,342`
source-data opens and reads at most
`3S+4A*F_source+2AN=15,313,780,736` aligned source bytes. It traverses the directory tree
once and never re-enumerates it for sorted records. A crash may repeat that fixed
generation but cannot enlarge it or persist progress authority.

Required canonical fields must agree exactly. For an optional field, one known value
dominates missing; two different known values are conflict. Volatile diagnostics are
omitted. Recovery-bearing conflict blocks cutover. Purely historical conflict emits
no ambiguity frame: it saturates every possible novelty bit for the reconstructable
affected episode or installs `UnknownIntegrityEvidenceFence` when identity is not
reconstructable. The classifier never chooses source order, truncates, or spills.
Semantic derivation precedes the initial Capsule quorum. A crash discards transient
descriptors and restarts the deterministic one-path-at-a-time pass from sealed
authority; no attempt history or intermediate generation is durable.

A no-egress dry generation freezes the inventory digest, classifier/schema digest,
ordered semantic-set digest, `S_egress`, `F_egress`, exact archive length, exact
object count, and a fixed 258-entry length/SHA-256/state table of 10,320 bytes.
`S_egress<=S=2,151,809,024`, `F_egress<=F_source=16,383`, and
`F_egress*640<=10,485,120` give `L_actual<=2,162,294,144`. The 258 fixed positions provide
`258*8,388,608=2,164,260,864` payload capacity; every object adds exactly one
4,096-byte envelope, so the exact maximum remote cohort is
`L_actual+object_count*4,096<=2,163,350,912` bytes. The used prefix has
`object_count=ceil(L_actual/8,388,608)` (zero for zero payload); unused table/key
positions remain `Empty`. Frame-continuation metadata lives inside that
envelope and never changes the payload bound. Output beyond these bounds blocks
before publication; no object is uploaded before the set is frozen. Sealed source
remains available through every S3 acknowledgement and the complete remote
revalidation pass.

Every imported evidence id has exactly one logical owner: `ImportedLegacyOwned` in
the classified legacy stream. No imported frame is written to either arena. For
current risk, market, and system episodes, bootstrap seeds the matching Capsule
novelty disposition; recurrence is a successful no-op before upload, after upload,
after local deletion, and after remote retention expiry. Older episodes are placed
behind the reconciled closed-episode frontier before activation. Remote object state
never clears imported ownership. Both arenas start empty and can contain only
post-cutover canonical states that were not imported.

### 4. Stage all three voters

The migrator preallocates and zero-validates both fixed arenas; imported evidence
never populates them. On each full recovery device it encodes bootstrap slot A with
a distinguished zero-parent header and slot B as A's direct child with
`parent_digest=digest(A)` and byte-identical logical state. It stages a manifest
candidate selecting B on each full device and the matching two digest/parent witness
records plus checksum-protected selector candidate on the third device. The selector
names exactly one record; that fully synced record carries the exact digest, parent,
and configured witness device id. Device identity is external to the hashed logical
state, so both full B slots and the selected witness vote name the same digest and
parent.

The migrator syncs and reopens both A/B pairs, both arenas, both manifest candidates,
and the witness pair/selector candidate, then validates the complete staged graph.
A missing/corrupt selector or mismatch in its selected record is an abstention, not
permission to infer the other witness record; one full replica plus that W is no
quorum. A+B may rebuild and republish the selector-selected witness vote.
The zero-parent header is accepted only while the kernel-fenced legacy path and
migration lock prove this one transaction; it is not a normal repair shortcut. No
live provider effect is allowed in this phase.

After staging and while all legacy inputs are still quiescent, the migrator
recomputes every legacy inode/hash and repeats venue reconciliation. Any drift
discards staging and restarts derivation before a pathname or authority transition.

### 5. Fence old binaries atomically

Rollback must not create two authorities. The already inventoried, 4,096-byte,
one-inode blocking directory is at the fixed same-parent staging name and was
created and parent-synced before the legacy quota was sealed. The migrator verifies
its inode/type/hash mapping, then atomically exchanges it with the configured legacy
JSONL path using `RENAME_EXCHANGE`. No allocation is permitted in this phase. After the exchange:

- the old path is a directory, so the legacy JSONL reader/writer fails closed rather
  than creating an empty file or replaying stale truth;
- the original legacy file is at the fixed retired path with the same hash;
- a parent-directory sync makes the exchange durable.

Before the exchange, only the lock-holding migration service may access the
quiescent legacy authority; the old runtime is already stopped and fenced. After the
exchange, only the new migrator/runtime can start. There is no missing-path window,
open legacy writer, or stale inode writer. Unsupported service, process, open-FD, or
filesystem fencing blocks before authority mutation. Restart accepts exactly two
inode/name mappings: source-at-original plus blocker-at-staging, or blocker-at-
original plus source-at-retired. Missing, mixed, or substituted identities halt.
The blocker remains inside the permanent 2,684,354,560-byte/16,384-inode legacy
project after eligible source deletion; quota never shrinks below its exact
4,096-byte/one-inode fence claim.

### 6. Publish one three-voter bootstrap

Publication starts only after the old-binary fence and every staged byte is durable.
The migrator publishes and parent-syncs replica A's manifest, replica B's manifest,
and then the checksum-valid witness selector naming the fully synced exact
digest/parent/device-id record, each selecting the same logical B digest and parent.
Although any matching pair is a normal commit quorum, bootstrap activation requires
all three votes plus byte-identical arenas. One or two published voters remain a
migration-only state and cannot authorize runtime startup or any provider effect.

Before any voter is published, the exchanged blocking directory plus both valid A/B
pairs and the valid witness pair/selector candidate is the unique resumable marker.
A crash after one or two publications validates the already published vote and completes only the
missing publications. A conflict, device-id mismatch, source-fence drift, or arena
mismatch halts; generation/device priority never chooses a winner. A missing/corrupt
witness selector remains an abstention even if either record's bytes match. Once all
three voter selectors/manifests are synced, `CapsulePublished` is the one-way
cutover marker and normal same-digest two-vote rules apply. Risk increase still requires all three voters and
both arenas; a two-vote quorum containing a full replica may durably continue only
non-risk-increasing workflow. There is no fictitious pre-cutover quorum.

The new runtime contains no JSONL recovery reader/writer and validates that the
blocking legacy path remains present. Release tooling refuses a pre-Capsule binary
on a Capsule-formatted state directory. Before complete three-voter publication, a
crash restarts only the migrator. After it, every restart uses the quorum-selected
Capsule even while retired history remains local. Production publication is
mechanically disabled. `AO-MIGRATION` lands only after `AO-CAPSULE` and `AO-HOST`
(and therefore #763, `AO-ROLLOVER`, `AO-BUDGET`, and `AO-NT.b`) and may exercise the
cutover only in hermetic fixture tests; its units expose no production invocation.
`AO-INTEGRATION` removes/source-fences the final legacy code path and exposes one
production-capable migration unit and one autonomous profile, both disabled by
default. It wires only `ACTIVATE-001..003`, which land disabled and crash-tested in
`AO-MIGRATION`; it creates no new durable edge. No PR, CI job, deploy helper, or
verification command invokes production cutover or profile enablement. A later
operator-run action is accepted only after all uploader, lifecycle-owner,
archive-lock, witness, replica-repair, host, review, and provider-capability gates
are green. No state permits simultaneous writable legacy and Capsule authority,
and there is no configuration switch back to JSONL.

### 7. Retire history automatically

`AO-MIGRATION` records only the final frozen inventory/classifier/semantic-set
digests, object table, and one numeric object cursor in the initial Capsule. #763
supplies the only Rust S3 uploader for both arena history and this one-time path.
There is no second worker, SDK configuration, retry table, staging directory, sort
scratch, or merge run. The versioned legacy classifier and source-metadata format are
immutable wire protocols while a migration can remain pending; source-fence
verification rejects a release that could not regenerate the frozen bytes after an
arbitrarily long outage.

The separate exclusive archive lock is exactly
`MigratorHeld -> TransferPrepared -> RuntimeHeld`. The migration service remains
the only archive writer while preparing the exact uploader/runtime generation; the
new owner cannot activate until the migrator is quiescent and `RuntimeHeld` is
quorum-durable. Restart at each phase reconstructs one owner, never zero or two. The
generated unit denies every other process write access to retired roots, and the
lock remains through final deletion. Autonomous capture paths are invalid.

The inventory is a lexicographic traversal of at most `F_total=16,384` total inodes,
including exactly one blocker and at most `F_source=16,383` source paths, with
normalized relative paths bounded by 512 bytes. Its fixed 64-byte descriptors use
exactly 1,048,576 bytes and index the 10,485,760-byte 640-byte source metadata table
that stores the complete root-relative reopen path. It folds
`(relative path, type, logical length, SHA-256)` and
configured sealed-root device identities into one digest; symlinks, nested mounts,
and bind aliases are invalid. The fixed migration section records only final
inventory/classifier/semantic-set digests, archive length and object count, the
10,320-byte expected-object table, one numeric object cursor, one prepared
descriptor, acknowledgement state, imported ownership masks, and
deletion/retention phase. Traversal and attempt history are never durable identity.

Exact-byte-egress-allowlisted Feather/raw history is validated and archived
unchanged. Registered JSONL is parsed for Capsule import and emitted only as a
classified binary stream of exactly the same length; raw JSONL bytes do not egress.
Unapproved raw paths are fixed quarantine and contribute neither remote bytes nor
metadata. Therefore:

```text
source ceiling            S = 2,151,809,024
egress bytes       S_egress <= S
egress metadata   640*F_egress <= 10,485,120
archive payload    L_actual <= 2,162,294,144
objects  ceil(L_actual/8,388,608) <= 258
payload capacity     258 * 8,388,608 = 2,164,260,864
remote bytes      L_actual + objects*4,096 <= 2,163,350,912
```

Together with the fixed market-ring cohort, S3 is capped at at most 1,261,698
objects and 3,314,119,482,752 bytes.

Object keys are fixed, never-reused `legacy/{000..257}`. Each object has payload at
most 8,388,608 bytes plus exactly one 4,096-byte envelope; any continuation metadata
is inside that envelope and cannot enlarge the payload. Before conditional PUT, a
two-vote Capsule transition containing at least one full replica commits
`ObjectPrepared` with numeric object cursor, payload length, and expected digest. A
crash or lost response regenerates the same bytes directly from sealed input and
metadata into the one 8,392,704-byte buffer. `PutUnknown` performs `HEAD`: exact
length/checksum acknowledges; absence retries; conflicting content is
delete/list-absent/recreated only under the verified exclusive never-versioned
bucket. `Acked` is quorum-durable before the cursor advances. No source path is
deleted while any object is unacknowledged or outcome-unknown.

The used prefix of the 258 fixed legacy positions occupies the dedicated cohort;
unused suffix positions remain `Empty`. The cohort cannot enter
365-day prune state while upload or revalidation is incomplete. After all are
acknowledged, the `RuntimeHeld` uploader holds its exclusive prefix lock, rechecks
the complete local inventory digest, and walks the numeric object table. Each
`HEAD` must reproduce fixed key, length, and SHA-256; a miss or conflict returns the
same object to pending and restarts bounded revalidation. Only a complete remote
pass, identical selected state on both full replicas, matching selector-valid
witness vote, no prepared/unknown PUT, and continuing archive fence permit
all-three `DeletionAuthorized`. That starts the legacy cohort's 365-day clock.

The archive owner then clears immutable protection only for classified/exact-byte
egress paths and performs idempotent descriptor-rooted deletion with
`beneath`/no-symlink/no-mount semantics. Quarantine paths remain immutable, sealed,
and present. It validates every frozen root and both egress/quarantine inventories
before each restart. A crash resumes deletion of remaining egress entries; upload
cannot be skipped because all-three `DeletionAuthorized` was durable before the
first unlink. `LocalEgressDeleted` commits only after every egress path is absent,
all affected parents are synced, and every quarantine path still matches its fixed
length/digest/allocation.

Remote deletion is eligible only when both the 365-day deadline and durable
`LocalEgressDeleted` hold. The S3 worker deletes the fixed keys in bounded batches, lists
the legacy prefix empty, commits registered `S3-COHORT-008` to `EmptyVerified`,
and then commits `S3-LEGACY-001: EmptyVerified -> Retired`; the namespace is never
reused. Imported
novelty ownership and the closed-episode frontier do not reset at remote deletion.
S3 outage retains fixed egress sources, fixed quarantine, the object table, and one
retry owner; no raw capture is produced and no memory/disk backlog grows.

Unsupported, corrupt, or classification-expanding source is never sent and never
used for recovery. It remains in a bounded local quarantine counted inside the
2,684,354,560-byte allocated legacy claim, with one fixed operational health alert
but no canonical evidence. Once
Capsule/venue reconciliation is complete, such purely historical quarantine does
not prevent management of risk or autonomous entry because its entire lifetime disk
claim is already reserved; it is a rare operator incident, not a normal repair
dependency. A corrupt recovery fact still follows the conservative quorum/venue
rules before entry can reopen.

## Compatibility Impact

- `shadow_pnl_report` and every other JSONL consumer found by the source census move
  to one bounded Rust export interface over classified arena/S3 history. The
  existing whole-file/unbounded JSONL report reader is removed, and the Python
  decision-evidence migrator is deleted and source-fenced only in
  `AO-INTEGRATION`, after disabled replacements and `AO-MIGRATION` fixtures exist.
  They do not receive a compatibility JSONL recovery writer or alternate migration
  command; prior PRs leave the old runtime authority unchanged.
- `nautilus.load_state`, `nautilus.save_state`, streaming capture, raw live capture,
  and file logging are invalid in the autonomous profile.
- Kill-switch state, recovery audit state, capital reservations, active strategy
  exposure, settlement, lifecycle, and evidence novelty commit through the Capsule.
- Unknown newer Capsule versions block. A schema upgrade writes the inactive fixed
  slot on both full replicas and publishes the same digest through the quorum
  protocol; no two version readers remain active.
- Reverse migration is an explicit stopped-node engineering operation requiring a
  new reviewed release and operator approval. It is not a normal recovery path or a
  runtime toggle.

## Exact PR Dependency Graph

The logical graph is exact and frozen. Numeric binding is administrative: it may
not change scope or edges. After Claude approval and explicit owner confirmation
for the issue mutation, numeric issue mapping and bidirectional relations are
required before implementation.

```text
AO-0  Design, invariant, migration and verification contract + Claude APPROVE
 ├── #1354  Closed state registry and non-recovery A->B->A suppression
 │     └── #883  Closed identifier classification and pre-arena pseudonymization
 ├── AO-BUDGET  Typed TOML resource ledger and protected acquisition classes
 ├── AO-NT.a  External nautilus_trader bounded-provider primitives/hooks PR
 └── AO-REDEEM  Disabled Rust-native relayer/SAFE redemption primitive

AO-BUDGET + AO-NT.a
 └── AO-NT.b  Bolt pin, bounded provider adapters/hooks and source fences

#1354 + #883 + AO-BUDGET + AO-NT.b + AO-REDEEM
 └── AO-CAPSULE  Capsule, fixed arena, durable authorization/replay and recovery

AO-CAPSULE + #883
 └── #763  Rust S3 archive, retention and automatic backlog recovery

AO-CAPSULE + AO-NT.b + AO-REDEEM
 └── AO-ROLLOVER  Observed rollover, cache retirement and redemption convergence

#763 + AO-ROLLOVER + AO-CAPSULE + AO-NT.b + AO-REDEEM + AO-BUDGET
 └── AO-HOST  Project quotas, journald, capture/log cleanup and release retention

AO-CAPSULE + AO-HOST
 └── AO-MIGRATION  Disabled stopped-service migrator, in-memory freeze and cutover fixtures
       └── AO-INTEGRATION  Cross-slice proof and sole disabled migration/profile integration
```

### PR contracts and safe intermediate states

| PR | Exact scope | Dependencies | Why main remains safe |
|---|---|---|---|
| `AO-0` | These four durable design artifacts and independent Claude decision | None | Documentation only; implementation and autonomous mode remain blocked |
| `#1354` | Current-source producer census, closed TOML canonical-state registry/family ranges, stable market-episode key types, and A→B→A suppression only for records proved non-recovery-bearing | `AO-0` approved | Existing JSONL remains sole authority and evolving recovery records are never suppressed. Risk ordinals, all-kind receipts, durable novelty, and retirement remain wholly atomic in `AO-CAPSULE` |
| `#883` | Closed field-classification and pseudonym types, SSM key/version handling, sentinel tests, and enforcement for existing non-recovery logs only | `#1354` | This is the complete narrowed #883 outcome: it does not claim an arena boundary that does not yet exist or rewrite recovery ids. `AO-CAPSULE` defines the bounded boundary, `AO-MIGRATION` fixtures conversion, and `AO-INTEGRATION` integrates only the disabled final path |
| `AO-BUDGET` | Typed TOML ledger; 2,147,483,648-byte ordinary pool; 536,870,912-byte recovery arena; 536,870,912-byte physical ballast; exact 3,758,096,384-byte operational/4,026,531,840-byte hard main memory; hard native-thread cap 128 with separately bounded async tasks and exact `LimitSTACK=1,048,576`; 512 protected FDs (`80+136+64+32+64+48+88`); 14 Polymarket members under 64 global; generated immutable `NetworkFootprint` caps/populations HTTP 18/17, WS 16/11, origins 19/18, DNS/TLS 18/17, 12 protected/four ordinary HTTP, two DNS, 34 physical/30 protected sockets; generated main and host `NetworkLifetimeFootprint`; closed-form `N_main`/`K_host`; 64 recovery/32 ordinary retry owners; `MemoryMin=1,610,612,736`, host-wide zero swap, effective main-unit `TasksMax=128`, `LimitNOFILE=2048`; closed AMI unit/socket/timer and non-Bolt network census; wrappers/source fences | `AO-0` approved | Every full worst-case claim is acquired before work; 30 protected sockets reserve from ballast and four ordinary sockets from Ordinary. Unknown rows/terms fail pre-open, async tasks cannot create native threads, and autonomous profile remains disabled |
| `AO-NT.a` | Pure bounded primitives/hooks in the pinned NT fork: byte+item priority ingress; signed-order preparation; exact `GET /data/order/{orderID}`/`associate_trades`/trade/receipt/log/status recovery hooks; exact decimal/scaled-integer balances including dust and raw positions; generation-owned subscription writes/observations; tracked transports/tasks; HTTP/1.1 zero-idle/redirect/proxy/library-retry serial-dial hooks; frame/body/pool caps; safe cache purge; 21-order/11-position-or-claim overflow probes | `AO-0` approved | Bolt pin and active runtime are unchanged. No durable authorization/replay or alternate reducer lands; absence/404/intermediate wire status never proves no effect |
| `AO-NT.b` | Pin exact fork SHA and expose only disabled Bolt bounded-provider adapters/hooks; bind every queue/pool/retry/thread/FD/buffer/network-lifetime limit to `AO-BUDGET`; source-fence direct APIs; register every client/connect/spawn/raw socket; supply whole-generation close/join and make per-asset unsubscribe unreachable to the autonomous profile; source-fence exact order/trade enum semantics and finalized-chain recovery | `AO-NT.a`, `AO-BUDGET` | Old supervised authority remains active and autonomous adapters stay disabled. No durable authorization/replay, caller migration, or second active lifecycle owner exists; `AO-CAPSULE` later owns all durable use |
| `AO-REDEEM` | Pure disabled Rust-native Polymarket relayer/SAFE primitive. Source-fenced manifest+TOML bind target/collateral/ABI/output asset and Safe internals; build/query exact original plus zero-value `nonce()` same-nonce fence bodies; source-fence explicit competing-nonce relayer acceptance; grouped SSM-only credentials | `AO-0` approved | It has no durable state, active caller, hardcoded address, or settlement shortcut. Main behavior is unchanged until Capsule owns authorization/retry/fencing |
| `AO-CAPSULE` | One logical Capsule graph on two full replicas plus 16-KiB witness; fixed arenas/receipts; 10/20/10/13 maxima; 16-KiB account-global Safe nonce lane; two-vote non-increasing commits; all-three entry gates; sole durable signed-order and redemption authorization/query/fencing; risk masks/retirement; disjoint legacy ownership; `UnknownIntegrityEvidenceFence`; exceptional source/finality/config/device/fence-bound `CatastrophicBootstrapCertificate`; and the sole transition registry/generator/sealed wrappers/generated crash matrix plus temporary hashed legacy-callsite census | `#1354`, `#883`, `AO-BUDGET`, `AO-NT.b`, `AO-REDEEM` | Lands disabled recovery authority/adapters only. Old JSONL remains sole active authority; census exemptions are unreachable from autonomous targets; neither provider primitive can act durably, and no migration or activation exists |
| `#763` | Same Rust binary in separate bounded archive unit; finite 365×3,456 market-key ring and 258-key/2,163,350,912-byte legacy slot; exact cohort ownership/empty-verification transitions; fixed envelopes/continuations; remote revalidation, ack-before-free, fixed retry and drain | `AO-CAPSULE`, `#883` | Bounded history sink only; no S3 recovery read, extra key, or local free on ambiguity |
| `AO-ROLLOVER` | Extend the sole Capsule-owned supervisor with 14 shared one-asset bundles, `Absent -> Requested -> Observed`, whole-generation replacement, source-fenced cancellation-safe provider operations, TOML join deadline with bounded self-termination/restart, exact `GammaMarketBinding`, separate non-temporal `EvidenceEpisodeId`, `DiscoveryHydrating`, pre-WS expiry transfer, bounded rebase, REST order ownership, Capsule-owned redemption convergence via `AO-REDEEM`, exact-state reconciliation, infinite capped retry, and cache retirement | `AO-CAPSULE`, `AO-NT.b`, `AO-REDEEM` | Adds policy to the disabled sole owner; per-asset unsubscribe is source-fenced unreachable and no primitive owns durable replay. Rollover/redemption transfer land together without an active dual path |
| `AO-HOST` | Provision seven project classes: full-recovery (two instances), witness, journal, report, release, legacy, system-mutable; one combined 8,192-B/two-inode active-release/runtime-mode selector pair; sole parent-directory lock and two-phase pre/target CAS; exact 256-B operator challenge in one phase-reused 4-KiB Ordinary union with public-key-only runtime verification; same-filesystem/exchange gate; registered restart-safe `SELECTOR-INIT-001..004`, `RELEASE-SWITCH-001..004`, and `DEV-EPOCH-001..007`; boot-volatile kernel-fence reinstall before voter reads; no migration scratch; exact disk/memory/journal/restart/alert/typed-health enforcement | `#763`, `AO-ROLLOVER`, `AO-CAPSULE`, `AO-NT.b`, `AO-REDEEM`, `AO-BUDGET` | No alternate token class or migration invocation; direct launch is atomically adopted then fenced, every release/device change selects `CapsuleDisabled` and clears stale autonomy approval, unavailable approval leaves current risk management running, and drift fails closed |
| `AO-MIGRATION` | Mechanically disabled stopped-service Rust migrator; sealed-input inventory; raw length-preserving archive plus in-memory semantic descriptors; no scratch/merge passes; exact 134,217,728-byte workspace and 258-object table; kernel/open-FD/old-binary fences; `MigratorHeld -> TransferPrepared -> RuntimeHeld`; three-voter bootstrap/cutover fixtures; disabled `ACTIVATE-001..003` over the existing combined selector pair, with `ACTIVATE-001` atomically binding the exact integration release | `AO-CAPSULE`, `AO-HOST` | Old runtime remains sole active authority. No production invocation exists, every archive-lock/selector phase has one writer, activation edges are unreachable outside hermetic fault tests, and legacy/Capsule cannot both be writable |
| `AO-INTEGRATION` | Cross-slice model/crash/restart/dependency tests, config cleanup, exact accounting report, root/backtester exact-head gates, final adversarial reviews, removal/source-fencing of the final legacy code path, empty-census/reachability/link-marker proof and census deletion, and one production-capable but disabled migration/profile integration | Every prior node, explicitly `AO-MIGRATION` | No PR, test, or deploy helper invokes production cutover or enables the profile. Provider capability gates remain fail-closed; later execution requires the engineering `AUTHORIZED` ruling and separate operator approval |

Each Bolt implementation PR starts from fresh then-current `origin/main`; stacked
branches are not reused after a dependency merges. `AO-NT.a` and `AO-NT.b` are
explicit slices of one issue because the fork has issues disabled; both PR bodies
name the remaining slice. No PR closes a broader issue unless its diff satisfies the
whole amended contract.

## Issue Drafts Requiring Owner Confirmation

The following are proposed issue contracts. They are not created or edited until
the owner confirms the batch.

### New `AO-0`: Prove bounded continuous autonomous operation

**Problem:** v0.1.13 exceeded its 1 MiB recovery read ceiling in five minutes and
has unbounded evidence, capture, lifecycle, and host-resource paths. A soak cannot
prove unbounded-lifetime correctness.

**Outcome:** Commit the finalized architecture, invariant/resource tables,
crash/dependency matrices, migration, PR graph, and verification contract at a
pinned current-source SHA; obtain independent Claude `APPROVE` before implementation.

**Non-goals:** implementation, deployment, EC2 start, live canary, signal/sizing/
profitability changes. This issue gates every node below.

### Amend `#1354`: Stable semantic evidence, not process-local per-tick dedupe

For this safe pre-Capsule slice, replace volatile dedupe only where records have no
recovery role:

- census every producer and recovery reader, then assign every producer to the
  frozen risk/market/system TOML family/id allocation;
- add typed `EvidenceEpisodeId` excluding prices, timestamps, exact slug, trusted
  open/close window, diagnostics, transient flags, schema/config/deployment identity,
  and retry counts; bind only stable logical strategy/target/venue,
  market/condition/question ids, ordered outcome/token identity, and applicable risk
  ordinal;
- apply finite A→B→A suppression only to producers the census proves are not used to
  reconstruct changing reservations, orders, fills, positions, or settlements;
- keep every recovery-bearing JSONL update unsuppressed because its payload may
  evolve within one phase;
- run large A→B→A tests for the safe slice and registry validation tests.

Risk ordinals, fixed 13-slot ownership, all-kind receipt novelty, retirement,
rotation, S3, Capsule storage, and recovery/evidence separation land atomically in
`AO-CAPSULE`. Relation: blocked by `AO-0`; blocks #883 and `AO-CAPSULE`; makes no
restart or all-kind exact-once claim early.

### Amend `#883`: Classify before immutable local evidence

Narrow #883 to the complete prerequisite it can safely deliver before the arena:
the closed classification registry, stable HMAC pseudonym types, SSM key/version
handling, sentinel tests, and enforcement on existing non-recovery logs. Sensitive
fields are allowlisted, pseudonymized, or omitted; the still-authoritative JSONL
recovery path is not rewritten.

`AO-CAPSULE`, not #883, defines the later enforcement boundary at evidence-frame
construction: raw operational ids stay in bounded workflow sections and active-risk
pseudonyms/non-secret key version are stored as payload. `AO-MIGRATION` implements
and fixtures the disabled registered-JSONL classifier, which omits sensitive
identifiers so regeneration cannot depend on retaining an old HMAC key. A
length-preserving raw family is uploadable only when the closed registry approves
its exact bytes; otherwise the whole family remains bounded local quarantine. Only `AO-INTEGRATION`
integrates the pseudonymous post-cutover projection, removes/source-fences JSONL
code, and leaves the registered production switch disabled. This division is
explicit issue scope, not a partially completed #883
outcome.

Relations: blocked by `AO-0` and #1354; blocks `AO-CAPSULE` and #763. Upload
mechanics remain #763.

### New `AO-BUDGET`: Establish the single application resource ledger

**Problem:** Capsule admission cannot reserve memory, tasks, descriptors,
endpoint-owner rows, queues, or retry ownership if those token classes arrive in a later
host PR; placeholders would create a dual path.

**Outcome:** add the typed TOML schema and one application ledger; a
2,147,483,648-byte ordinary pool; a touched 536,870,912-byte recovery arena; and a
536,870,912-byte physical ballast whose maximum substitutions are 134,217,728 bytes
of resident stacks, 188,743,680 bytes across `30*6,291,456` protected sockets, and
213,909,504 permanently locked bytes. Recovery/config page cache is owned only by
`N_main`, never ballast. Main
operational/hard memory is exactly 3,758,096,384/4,026,531,840 bytes with
`MemoryMin=1,610,612,736`, host-wide `MemorySwapMax=0`, effective main-unit
`TasksMax=128`, `LimitNOFILE=2048`, and `LimitSTACK=1,048,576`. Add byte+item queues;
separate async-task partitions of
512/384/128 hard/ordinary/recovery, while native threads have an independent hard
maximum of 128 because only `128*1,048,576` stack bytes are reserved. The protected
async-owner count is
`10*6 + 16*2 + 8*3 + 12 = 128`; 2,048/1,536/512 FD partitions;
14 Polymarket wire asset ids under the unchanged 64-member global subscription cap;
64 protected/32 ordinary retry owners; and generated `NetworkFootprint` from
resolved TOML. Its cap/population pairs are HTTP 18/17, WS 16/11, origins 19/18,
and DNS/TLS 18/17; HTTP live partitions are 12 protected/four ordinary,
with two DNS sockets, physical max 34, and protected max 30. One WS physical socket
per owner follows from close/join-before-redial. NT uses HTTP/1.1, zero idle,
redirect, proxy, and library retry, and serial dial. Every connect, client, spawn,
and raw socket consumes generated rows before open; unknown paths block.

Generate `NetworkLifetimeFootprint` from the same resolved TOML. It disables and
verifies autotune; enumerates without an opaque remainder every effective
`SO_RCVBUF`/`SO_SNDBUF`, TLS/user buffer, socket/skbuff/kernel-object multiplicity,
route/neighbour entry, DNS UDP/TLS cache, and connection object whose per-live-socket
sum is exactly `C=6,291,456`. Thirty protected sockets reserve full `C` from ballast
and four ordinary sockets reserve full `C` from Ordinary before open. Global
protected/ordinary dial buckets and per-owner minimum reconnect/stable-reset rules
bound `TIME_WAIT`, FIN/orphan, conntrack, and ephemeral-port occupancy by
`retained <= concurrency * (ceil(H/min_dial) + 1)`. At close, the signed-kernel
charge map moves main-cgroup residue into disjoint `N_main.net_retained` rows and
only root/unmanaged residue into `K_host`; `C` is retouched or reused only after
every byte has a retained owner, and neither retained claim releases before its
effective counters prove uncharge.

Generate disjoint ledgers
`N_main = native-thread guard/VMA/page-table metadata (resident stack pages excluded) + ELF PT_LOAD + pinned DSO PT_LOAD +
loader/vDSO/static TLS/mappings + VMAs/page tables from declared virtual mappings +
fixed allocator arenas/metadata + recovery/config page cache + declared
runtime/control objects + process-attributed nonsocket kernel objects +
main-cgroup retained socket rows` and
`K_host = signed-AMI pinned base/kernel static + ceil(memtotal_max/base_page)*BTF
struct-page bytes + perCPU + per-device + global network/fs/cgroup + uncharged-only
journal/filesystem cache + root/unmanaged retained socket states`. Every coefficient
comes from TOML, build/kernel manifest, or signed AMI manifest, the two ownership
sets are disjoint, and their generated sums must fit the fixed caps. Missing terms
block; measured current charge is only drift evidence and never reduces pre-open
ballast.

**Failure behavior:** missing or unenforceable bounds keep autonomous mode invalid;
ordinary saturation preserves both recovery classes. No Capsule authority,
project-quota, or journald setting lands here. Relation: blocked by `AO-0`; blocks `AO-NT.b`,
`AO-CAPSULE`, and `AO-HOST`.

**Verification:** exact ten-risk ledger arithmetic, every partition boundary,
charged/touched arena and ballast, release/consume/retouch of each typed claim,
hard native-thread 127/128/129 and independent async-task boundaries, and exact FD
sum `80+136+64+32+64+48+88=512`. Prove every network cap/population, full pre-open
ordinary/protected reservation, per-term `C` sum with no slab, charge-owner close
transfer to `N_main.net_retained` or `K_host`, lifetime residue formula at
`limit-1/limit/limit+1`, ephemeral-port safety,
and closed HTTP/SDK pools. Verify HTTP/1.1 serial dial, zero idle/redirect/proxy/
library retry, exact disjoint `N_main`/`K_host` manifest derivation, swap-disabled
cgroup/unit, saturation while protected work runs, and source-fence every allocation,
client, connect, spawn, and raw-socket path.

### New `AO-NT`: Bound the Nautilus runtime boundary

**Problem:** pinned NT uses unbounded channels, untracked provider tasks, fire-and-
forget subscription calls, and caches without one Bolt lifecycle owner.

**Outcome:** two pure bounded-provider slices with no durable authority. (a) The fork
PR adds item+byte priority lanes, fixed pools, pre-send signed-order preparation and
pre-dispatch finalized-block hook; generation-owned subscription writes and
asset-specific observations; tracked transports/tasks; HTTP/1.1 zero-idle/redirect/
proxy/library-retry serial-dial hooks; safe purge; exact decimal/scaled-integer
balances including every positive dust value and raw redeemable/non-redeemable
position; and bounded current capture with 21-order/11-position-or-claim overflow
probes. Decode the POST-create response independently from GET-order: a valid POST
success envelope admits only lower-case wire values `live`, `matched`, `delayed`,
and `unmatched`, while transport failure, non-2xx, `success=false`, or a malformed
success envelope collapses to the frozen bounded `PostDiagnosticFailure` metadata
and is nonterminal. GET admits exactly `ORDER_STATUS_LIVE`,
`ORDER_STATUS_INVALID`, `ORDER_STATUS_CANCELED_MARKET_RESOLVED`,
`ORDER_STATUS_CANCELED`, and `ORDER_STATUS_MATCHED`. Trade decoding independently
admits only bare wire values `MATCHED`, `MINED`, `CONFIRMED`, `RETRYING`, and
`FAILED`; internal enum names are generated separately and never accepted as wire
aliases. The route fixture must resolve the current POST-schema/lifecycle-contract
drift over `unmatched`. Drift, an unknown/additional value, an unknown error field
or code, or a cross-endpoint value fails closed.

Both slices make signed order bytes, signatures, authorization headers, all SSM
credential values, and raw provider success/error/request buffers non-loggable and
non-serializable through observability traits. Only fixed redacted ids, lengths,
outcome classes, and digests may enter logs, evidence, reports, or alerts; this is
owned here for the CLOB boundary, not deferred to `AO-REDEEM`.

The fixed recovery primitive accepts only the exact
`ProviderTerminalCertificate`: a fill returns the complete sorted unique at-most-
64 set of canonical 32-byte transaction hashes for sequential at-most-2,097,152-
byte/4,096-log receipt/indexed-`OrderFilled`/V2-status/post-state verification; a
no-effect terminal is an indefinite linearizable exact-hash tombstone against all
submit/delay/retry/match/duplicate/preapproval work. Individual negative signals,
ordinary status, cancel response, 404, elapsed time, unsigned expiration, and quiet
chain never release or replay. No uptime-sized log or unfiltered account-wide
terminal-history query exists. The current V2 capability fixture must fail because
no permanent maker-controlled tombstone or reviewed complete-at-most-64 hash-set
contract exists; either failure mechanically blocks the autonomous profile until a
pinned provider revision supplies both contracts.

(b) Bolt pins the exact fork SHA, supplies every limit from TOML and `AO-BUDGET`,
and exposes only disabled bounded adapters/hooks. Every HTTP/WS/DNS client, connect,
spawn, and raw socket is registered through both generated network footprints;
whole-generation close/join is authoritative and the existing send-only per-asset
unsubscribe receives no correctness credit. Direct APIs are private/source-fenced.
No caller migration, durable prepare/query/replay phase, lifecycle policy, or
authorization lands in either slice; `AO-CAPSULE` later owns all durable use. The
venue exposes no reviewed stable-id close-until-flat primitive, so there is no
alternate reducer or booking-association reconstruction.

**Failure behavior:** bounded hooks return typed overflow/Unknown and never infer
absence, terminality, replay authority, or release. Autonomous adapters remain
disabled; failure of any required capability blocks both slices and final operation.

**Verification:** exercise every route/status/cap/terminal-certificate negative and
positive boundary above. Compile-fail and source-census every CLOB observability
conversion, then inject sentinel signed requests, signatures, authorization
headers, SSM values, and successful/failed/malformed/oversize raw responses through
captured journal/evidence/report/alert sinks; require no sentinel substring and
exactly the allowed redacted fields.

**Non-goals:** trading signals, lifecycle policy, and durable authorization/replay.
Relation: `AO-NT.a` is blocked by `AO-0`; `AO-NT.b` by `AO-NT.a` and `AO-BUDGET`;
`AO-NT.b` blocks `AO-CAPSULE`, `AO-ROLLOVER`, and `AO-HOST`.

### New `AO-REDEEM`: Add the disabled Rust-native relayer/SAFE primitive

**Problem:** existing settlement bookkeeping is not proof that a Polymarket claim
was redeemed, and a non-queryable or reconstructed call cannot be crash-safe.

**Outcome:** add one pure, mechanically disabled Rust-native primitive. A
source-fenced provider manifest plus grouped TOML binds chain, wallet type, SAFE
address, target, collateral, ABI, output asset, and SSM-only relayer credentials;
missing/mismatch blocks and no address, amount, or credential source is hardcoded.
For both modes, target the current V2 collateral adapter's inherited external
`redeemPositions(address,bytes32,condition,uint256[])` ABI with exact manifest-fixed
dummy values source-fenced as ignored. Standard internally burns through CTF;
negative-risk internally derives wrapped-collateral current balances, calls the
legacy `INegRiskAdapter.redeemPositions(condition,amounts)`, and wraps USDC.e to
current pUSD/PMCT. Bolt never targets that internal two-argument ABI. Retain an
exact two-balance pre-state and exclusive condition mutation lease, and revalidate
both immediately before send; pre-send drift aborts/reprepares, while post-dispatch
drift reconciles from exact logs/post-state under the same Safe body.

Expose pure builders for two complete explicit-nonce relayer bodies: the original
redemption and one manifest-bound, zero-value same-nonce Safe transaction whose
inner call invokes the source-fenced Safe proxy's side-effect-free `nonce()` getter.
The deterministic identity is `(chain, wallet type, SAFE address, SAFE nonce,
target, calldata hash)`; retry accepts only identical body bytes. Query the exact
relayer transaction id when known, both Safe transaction hashes, on-chain nonce,
finalized receipt/log, and raw post-balance/claim. `NEW`, `EXECUTED`, `MINED`,
`FAILED`, and `INVALID` are observations only. `CONFIRMED` succeeds only with chain-
state confirmation. A fence succeeds only when its finalized execution consumes
the nonce and claim/post-balance is unchanged; an unrelated nonce consumer is
integrity failure. Source-fence relayer acceptance of an explicit competing same
nonce; missing support blocks the profile.

**Intermediate safety:** the primitive owns no durable state, has no active caller,
and remains disabled. It cannot infer a result from settlement bookkeeping or
provider absence. `AO-CAPSULE` is the only later owner of durable authorization,
query/replay, and capacity release.

**Verification:** source-fence every provider-manifest/TOML binding and exact ABI;
fixture both market modes and every scaled-integer/dust boundary; construct the
original and same-nonce fence at every size edge; fixture original-wins, fence-wins,
and unrelated-nonce-consumer outcomes; prove explicit competing-nonce relayer
acceptance, byte-identical retry, SSM-only credential resolution, and static absence
of a hardcoded address or active invocation. Prove at compile/source-fence and
captured-log boundaries that no signed body/request, signature, authorization
header, credential, or raw provider response/error can reach logs, evidence, or
alerts. Relation: blocked by `AO-0`; blocks `AO-CAPSULE`,
`AO-ROLLOVER`, and `AO-HOST`.

### New `AO-CAPSULE`: Make local recovery and evidence capacity fixed

**Problem:** mixed append-only JSONL is an unbounded, torn-write-sensitive authority
whose read cap can discard required risk truth.

**Outcome:** one logical Capsule graph on two independent full replicas plus the
16,384-byte digest witness on a third device; two 31,457,280-byte mirrored arenas;
10 risks/20 orders/10 settlements/13 episodes; fixed receipts and
non-replenishing novelty/ordinal masks; and the all-kind workflow/evidence split.
Implement same-digest two-vote commits containing a full replica for
non-risk-increasing work, all-three plus both-arena agreement for risk increase, and
automatic stale-voter repair. W votes only through a checksum-valid selector naming
one fully synced exact digest/parent/device-id record; missing/corrupt W abstains,
A+B repairs it, and one full+invalid W is no quorum. Entry commits fixed
request/capacity as `EntryPreparedNotAuthorized`, then separately all-three
authorizes or aborts after a same-candidate recheck; dispatch-unknown replay needs a
fresh all-three authorization and identical signed bytes/hash. Capsule also owns the
sole durable redemption authorization/query/replay around `AO-REDEEM`'s exact SAFE
nonce/body identity; primitive and settlement bookkeeping have no independent
authority. Order phases retain the pre-dispatch finalized block and fixed hash
recovery cursor; no negative/404/intermediate wire state releases capacity.

The PR supplies fixed migration schema, legacy ownership states, 258-entry object
table, and disabled adapter interfaces, but performs no inventory, cutover,
legacy-reader removal, or invocation. Loss of both full media/all four payload slots
is catastrophic, not ordinary repair. Ordinary recovery imports no aggregate and
initiates no effect. The only sentinel-root path is a
`CatastrophicBootstrapCertificate` over exact bounded current aggregates plus
source/finality, resolved-config, replacement-device, and exclusive-account-fence
digests, published in exact A then B then witness order. Aggregate/fence drift
invalidates partial publication and old lineage cannot vote. A complete sentinel
root remains `HaltedUnknownIntegrity`, emits no canonical evidence, and permits only
reduction to zero/redemption of the exact unattributed aggregates until a separate
authenticated policy/association repair completes. S3 and venue absence cannot
clear it.

This PR also owns `ci/autonomous-transitions.toml`, its closed generator and sealed
durability/effect wrappers, the bounded generated durable-transition crash matrix,
and the temporary legacy-callsite census. Each census row has a stable callsite id,
fully qualified symbol plus lexical AST ordinal, versioned canonical/raw node
digests, effect class, and removal PR. Unique link markers and a closed target/call
graph prove that no autonomous entrypoint reaches a census row; unknown dynamic
edges fail the build. Later PRs may only delete census rows as they register their
effects.

The separate `UnknownIntegrityEvidenceFence` is only for recoverable corrupt-history
cases with trustworthy Capsule authority. It saturates affected novelty families
without becoming a catastrophic bootstrap or entry-clear mechanism.

**Verification:** exact size/cardinality, `10/20/10/13` and 20/21 account-capture
boundaries, reserve exhaustion, every voter/selector crash row, prepared/authorized/
aborted/replay and redemption phase crashes, repair-never-authorizes, exact order-
recovery and release gates, corrupt-state/venue matrices, unknown-integrity fence
recurrence/serial-wrap properties, and catastrophic certificate crash/drift/
lineage/authenticated-repair cases. Relations: blocked by #1354, #883, `AO-BUDGET`,
`AO-NT.b`, and `AO-REDEEM`; blocks #763, `AO-ROLLOVER`, `AO-HOST`, and
`AO-MIGRATION`.

### Amend `#763`: Fixed outbox history sink; S3 is never recovery

Replace segmented JSONL/sidecar/options language with one same-binary Rust archive
worker in a separate unit bounded to 201,326,592 operational/268,435,456 hard memory
bytes, a 32-permit async semaphore distinct from OS `TasksMax=16`, 64 FDs, two
origins, one sequential HTTP/1.1 live connection, and zero idle, redirect, proxy,
or library retry. Require the finite `365*3,456` market-key ring, fixed
258-key/2,163,350,912-byte legacy slot, deterministic objects (at most 12/market),
conditional PUT to final fixed keys, classified envelopes, checksum/length
verification, quorum-durable acknowledgement and local-reference barrier before
slot reuse, and exact registry
`Unverified -> EmptyVerified -> Owned -> ReuseBlocked -> DeletePrepared -> Deleting
-> VerifyingEmpty -> EmptyVerified -> Owned`. Before any first ownership, one fixed
cursor performs exactly 366 `ListObjectsV2(MaxKeys=1)` empty-prefix checks; an
unexpected key integrity-halts before PUT. Every payload is at most 8,388,608 bytes plus exactly one 4,096-byte
envelope containing any continuation metadata. Require capped retry,
used-legacy-object remote revalidation plus unused-key absence, all-three
`DeletionAuthorized`, `LocalEgressDeleted`, explicit delete/list/empty barriers,
registered `S3-LEGACY-001: EmptyVerified -> Retired`, permanent local quarantine
outside deletion, and support for archive-lock
handoff `MigratorHeld -> TransferPrepared -> RuntimeHeld` in a dedicated bucket that
has never had versioning enabled.
S3 is history only and is never read on restart. An outage fills the fixed local cap,
blocks new risk before reserves, and automatically drains/resumes.
The combined market-ring plus legacy ceiling is 1,261,698 objects and
3,314,119,482,752 bytes.

Ordinary logs and ongoing runtime capture policy move to `AO-HOST`; the bounded
one-time legacy uploader remains in #763. Relations: blocked by `AO-CAPSULE` and
#883; blocks `AO-HOST` and `AO-INTEGRATION`.

### New `AO-ROLLOVER`: Converge rollover and settlement without leaks

**Problem:** role-based calls can re-subscribe an expired position instrument and
provider tasks/cache entries lack generation-owned observed lifecycle state.
Settlement can stop retrying.

**Outcome:** extend the sole Capsule-owned supervisor, using only `AO-NT.b` hooks,
with 14 durable composite
bundles, each owning at most one Polymarket asset id/wire member shared by book and
trade consumers, under the unchanged 64-member global cap. Every generation starts
all members `Absent`, moves one to `Requested` only after transport write, and moves
it to `Observed` only after the first valid current-generation asset-specific full
book snapshot or source-fenced sequence-complete baseline. Delta/trade before that
baseline leaves it `Requested`, invalidates the book, and requests one resnapshot;
Polymarket has no server subscription ACK and send success is not one. Any desired-set change or observation timeout closes/joins the whole old market
generation and opens a fresh generation with the exact complete desired set and no
expired asset. The existing per-asset unsubscribe is send-only and receives no ACK;
the autonomous-profile source fence makes it unreachable. Entry requires every target
`Observed` plus a fresh complete book, while REST remains authoritative for orders.

The same owner performs pre-WS restart expiry transfer and exact-slug Gamma
discovery. It derives one immutable slug from TOML template + lane + trusted window,
queries `slug=<exact>&limit=2&offset=0` with bounded body/items, and waits in one
`DiscoveryHydrating` retry state without an episode while either token id is
missing. The first fully hydrated result binds a `GammaMarketBinding` exactly as
`(gamma_market_id, condition_id, question_id, exact_slug, trusted window open/close,
neg_risk_mode, ordered exactly-two [(outcome_index, normalized_outcome,
clob_token_id)])`; later mutation blocks rather than rebinding. This lifecycle
binding is not evidence identity. `EvidenceEpisodeId` excludes slug, window,
timestamp, and all transient values; it uses stable logical strategy/target/venue,
non-temporal market/condition/question ids, the ordered outcome/token binding, and
risk ordinal only for a family that needs it. Slug/window churn cannot reset novelty;
only a genuinely new market/condition id can roll the market episode. Two items,
cap overflow, or wrong identity is ambiguity. Zero after trusted close commits
`ClosedWindowNoAcceptedCandidate`--no new risk, episode, evidence, or historical-
absence claim--while account/risk reconciliation remains authoritative. Arbitrary-
downtime rebase uses at most the current+next exact-slug queries plus the existing
20 order/10 risk/10 settlement queries under one 30-second freshness lease. The
owner also provides modular contiguous frontier recovery, the complete provider-
backed lifecycle around Capsule-owned `AO-REDEEM` authorization/query/replay and
direct-parent slot repair; refcounted purge; and infinite in-place capped retry. It
never owns a second durable provider phase or infers absence.

**Verification:** 100,000 rollovers, downtime across expiry, missing/ambiguous
windows, all Gamma zero/one/two/overflow/wrong-identity outcomes, serial wrap, and
failure injection at every `Absent/Requested/Observed` and whole-generation phase.
Assert missing-token hydration without an episode, exact first binding, later-field/
ordering/token mutation block, and that arbitrary slug/window/timestamp churn cannot
reset `EvidenceEpisodeId` novelty while a new market/condition id does. Assert
shared one-member bundles, 14/64 caps, stable subscription/task/FD/cache/retry/client/
generation counts, bounded error rate with no storm, no claimed unsubscribe ACK or
expired subscribe call, fresh-book entry gating, bounded 30-second rebase, REST
order authority, and Capsule-only redemption phases.
Relations: blocked by `AO-CAPSULE`, `AO-NT.b`, and `AO-REDEEM`; blocks `AO-HOST` and
`AO-INTEGRATION`.

### New `AO-HOST`: Enforce all host and dependency resource bounds

**Problem:** application budgets alone do not enforce cgroup, filesystem, journal,
capture, report, or release limits on the host.

**Outcome:** extend and verify the renderer with the exact 8,053,063,680-byte
physical-memory claim: 4,026,531,840 main, 268,435,456 archive, 134,217,728 journal,
1,610,612,736 bounded system services, 268,435,456 user slice, 1,073,741,824 touched
sacrificial reserve, and a separate 671,088,640-byte unclaimed kernel reserve.
Enforce `MemTotal`, slice, `MemoryMin=1,610,612,736`, host-wide zero-swap,
closed unit/socket/timer task/FD/network census, reserve-service, and
unclaimed-gap gates. Provision project quotas on at most four devices; place the two
78,643,200-byte/16-inode full-recovery projects and the 65,536-byte/8-inode witness
project on three distinct devices; assign exactly seven project classes--the
full-recovery class (two instances), witness, report, journal, release, legacy, and
system-mutable--and no migration-scratch class or spill path. Seal legacy at
2,684,354,560 bytes/16,384 inodes. For every device, sum applicable future claims
but set `byte_floor(d)` to the maximum applicable class floor, never their sum:
10 GiB for any recovery/data class, otherwise 2 GiB for root/log; apply the 65,536
inode floor exactly once. The exact migration cold-data minimum is
13,438,550,016 bytes/81,924 inodes. The aggregate mutable-disk peaks are
3,462,463,488 bytes post-migration and 6,146,818,048 bytes during
migration or quarantine. Dedicate journald, disable autonomous raw/catalog/file logging, bound
reports/releases, deliver latest fixed health through one Rust-native single-flight
alert transport, generate `Restart=always`, TOML `RestartSec=30s`,
`StartLimitIntervalSec=0`, and journal rate limits, and expose typed self-clearing
host health. It does not add alternate application token types.

Inside the existing system-mutable ceiling, preallocate exactly one 8,192-byte/
two-inode current/staging selector pair whose fixed record combines active release,
immutable admission digest, device epoch, closed runtime mode, capability/review
digest, predecessor-selector digest, fixed action-specific authorization envelope,
and checksum.
One selector-mutator holds a nonblocking exclusive lock on the existing parent
directory FD continuously from first inactive-record mutation through record sync,
exchange, parent sync, and reopen verification or abort. It compares the complete
expected current and inactive-before digests plus inode/device mapping before the
first write. Immediately before exchange it requires the current/identities still
match and the inactive inode is the checksum/signature-valid prepared target;
crash releases and restart reacquires the lock. Source fences reject any
other writer; no lock inode or second selector exists. Before authority, prove
the pair is same-filesystem and supports `RENAME_EXCHANGE` using the pair itself.
The operator private key is governance authority, not a product/runtime credential:
it never enters Bolt, SSM, or the host. The exact 256-byte challenge and exact
512-byte response share one 4,096-byte Ordinary union workspace; current risk management continues
until a returned signature passes fresh-prestate verification.
That one 4,096-byte Ordinary union is phase-reused for the target, exact 256-byte
challenge, exact 512-byte Ed25519 response, and final reconstruction; only the
response temporarily occupies 512 bytes of the already-reserved mutator stack.
No second target/challenge buffer or uncharged memory exists.

Land registered `SELECTOR-INIT-001..004` to adopt the exact current legacy release:
stop/mask/drain, prepare/sync/exchange-test the non-authoritative pair, atomically
replace the masked direct launcher with the selector-only launcher, verify/unmask,
and permanently source-fence direct launch. Exact pre leaves only the stopped direct
authority; exact post selects the same legacy release through the selector; a
restart-enabled host transition supervisor automatically resumes the masked middle.
Land `RELEASE-SWITCH-001..004` for every later deployment. A changed release always
selects `CapsuleDisabled`, clears old provider/review/engineering/operator autonomy
approval, and retains existing-risk management until fresh `ACTIVATE-003`.

Land reusable `DEV-EPOCH-001..007`: drain the old runtime; stage/sync/reopen/
signature-check the immutable admission record; construct a full
`CapsuleDisabled` candidate with fresh device/compatibility/operator evidence and
no inherited autonomy approval; compare/exchange the fixed selectors; parent-sync
and accept only exact pre/post mappings; on every boot/start install and verify the
boot-volatile old-device kernel denylist before issuing a process-local voter-read
capability; allow capture but no A publication at step 006; and only after the
launcher/rollback proof at step 007 make A reachable. A restart always closes the
voter-read gate.

Render and verify the signed AMI/build/kernel coefficients for disjoint `N_main` and
`K_host`, including native-thread guard/VMA/page-table metadata (resident stack
pages excluded), ELF/DSO/loader mappings, VMAs/page
tables, allocator metadata, recovery/config page cache, process kernel objects,
signed base/RAM-page/per-CPU/per-device/global network/fs/cgroup terms,
route/neighbour and DNS UDP/TLS caches, main-cgroup retained socket rows in
`N_main`, and root/unmanaged retained socket states in `K_host`. Reject `MemTotal` outside
`[8,053,063,680,8,589,934,592]`. Missing terms or
sum overflow blocks; observed current usage cannot reduce the claim.

**Verification:** journal rotation overlap, quota/full/read-only faults, release and
report peaks, static/runtime proof of no scratch/spill, all seven project classes,
max-not-sum byte floors and one inode floor per shared/separate device, exact
post/migration/cold-data peaks, disjoint generated `N_main`/`K_host` sums,
capture/log prohibition, exact effective
cgroup/unit/project-quota inspection, every `SELECTOR-INIT-001..004`,
`RELEASE-SWITCH-001..004`, and `DEV-EPOCH-001..007` crash boundary; parent-lock
contention and stale expected-record interleavings; same-filesystem/exchange
failure; exact pre/post selector and launcher mappings; stale-media return after
each device boundary; zero voter opens before the boot fence; zero A publication
before step 007; challenge timeout/crash/rejection with current risk management
continuing; operator-key absence from runtime/SSM/host; stale authorization
clearing; and config/unit drift failure. Relations:
blocked by #763, `AO-ROLLOVER`, `AO-CAPSULE`, `AO-NT.b`, `AO-REDEEM`, and
`AO-BUDGET`; blocks
`AO-MIGRATION` and `AO-INTEGRATION`.

### New `AO-MIGRATION`: Land the disabled one-way cutover tooling

**Problem:** migration needs bounded sorting, compatibility fences, and bootstrap
crash recovery without giving an intermediate `main` revision a second writable
authority or an invocation path.

**Outcome:** add the mechanically disabled stopped-service Rust migrator. Under the
continuing kernel DAC/capability/mount/open-FD fence it seals at most
`S=2,151,809,024` source bytes, `F_total=16,384` inventory paths including the
blocker, `F_source=16,383` source-data paths, and `N=1,048,576`
semantic records. Exact-byte-egress-allowlisted raw history archives unchanged;
unapproved families are permanent quarantine and never uploaded/deleted;
registered at-most-2-MiB JSONL frames are semantic and emit only a
length-preserving classified binary history representation. Before stopping, the
old runtime must disable entry and prove authoritatively flat. Then require
single-link/no-writable-FD-or-MAP_SHARED/immutable/exclusive-read sealing. Stream one sealed path at
a time, with no scratch, run file, merge pass, or local object staging. Hold exactly
`F*64=1,048,576` virtual-range path descriptors, `F*640=10,485,760` complete-path
source reopen/egress metadata, and `N*40=41,943,040` semantic descriptors; sort by
digest and reopen via metadata index for one reread per descriptor.

Use the exact 134,217,728-byte workspace: 33,554,432 aligned direct-I/O input buffer + 41,943,040
semantic descriptors + 1,048,576 inventory + 8,392,704 object buffer + 33,554,432
Feather/decoder + 10,485,760 source reopen/egress metadata + 5,238,784 join/key/slack. Every source
read and equal-key reread uses `O_DIRECT`/`RWF_DIRECT`; preflight verifies
`STATX_DIOALIGN`, aligned sealed-tail handling, and no buffered fallback. Fadvise/
mincore proves zero payload-data cache; generated filesystem metadata fits
134,217,728 bytes. Enforce `N+2F_source=1,081,342` opens and
`3S+4A*F_source+2AN=15,313,780,736` aligned bytes. Freeze `S_egress`,
`F_egress<=16,383`, `L_actual<=2,162,294,144`, a
258-entry/10,320-byte table with unused `Empty` positions, payload capacity
2,164,260,864, and remote maximum 2,163,350,912; continuation metadata stays
inside each 4,096-byte envelope.

Derive byte-identical recovery/import/object state; stage selector-valid three-voter
bootstrap bytes; implement atomic old-binary/path fences, archive-lock handoff
`MigratorHeld -> TransferPrepared -> RuntimeHeld`, all-three
`DeletionAuthorized`, and `LocalEgressDeleted` before remote prune. The initial Capsule
freezes only final digests/object table, never sort attempts or traversal history.

Predeclare and crash-test disabled `ACTIVATE-001..003` against AO-HOST's one
combined current/staging selector pair: `Legacy -> Migration` requires the exact
action-specific operator authorization plus fresh flat certificate and atomically
selects the reviewed integration release; `Migration -> CapsuleDisabled`
requires selected three-voter bootstrap, both arenas, and `RuntimeHeld`; and
`CapsuleDisabled -> Autonomous` requires the exact green provider/resource/review
manifest, engineering `AUTHORIZED` ruling, and separate operator authorization.
Each authorization binds the edge, expected record/inode/device mapping, complete
target core, and prerequisite evidence. The sole selector mutator holds the parent
lock while it writes/syncs the inactive 4-KiB record, compares current, exchanges
the fixed pair, parent-syncs/reopens, and accepts only the exact pre/post mapping.
The production callsites remain
unreachable; hermetic fault tests invoke only sandbox roots.

The old runtime remains the sole authority when this PR lands. Its units expose no
migration invocation; no production path exchange, Capsule publication, legacy
reader removal, or autonomous activation occurs. Hermetic fixtures prove there is
never simultaneous writable legacy and Capsule authority.

**Verification:** path/frame/descriptor/workspace `limit-1/limit/limit+1`; exact
workspace and object arithmetic; raw length preservation and semantic-only JSONL
registration; digest-collision full-key reread; permutation-invariant semantic
state; no scratch/spill/merge/writeback path; mandatory direct I/O with zero source
page-cache ownership;
every source/voter/selector/path/archive-lock/upload/revalidation/deletion crash;
incident/corrupt fixtures; and static proof that invocation is impossible.
Relations: blocked by `AO-CAPSULE` and
`AO-HOST`; blocks `AO-INTEGRATION`.

### New `AO-INTEGRATION`: Prove and integrate autonomous operation

**Problem:** component tests and elapsed soak time do not prove the cross-component
invariant.

**Outcome:** add the complete model, crash, migration, dependency, restart,
retention, rollover, quorum-voter, deterministic-import, and reserve suites; produce
exact accounting; pass exact-head root/backtester/source-fence CI and final
adversarial/native review; remove/source-fence the final legacy runtime reader/writer
and Python migrator; require the temporary legacy-callsite census and both its
autonomous target-reachability/link-marker sets to be empty, then delete the census
in the same PR; and expose exactly one production-capable stopped-service
migration entrypoint plus one autonomous profile, both mechanically disabled by
default. Tests use hermetic roots only. No repository PR, CI job, deploy helper, or
verification command may invoke production migration or enable the profile.
Production cutover is a later operator action that is legal only after every
provider capability gate is green, the final report rules `AUTHORIZED`, and the
operator separately approves deployment/EC2 start/live activity. The entrypoint
still enforces the atomic path fence, so legacy and Capsule can never be
simultaneously writable when that later action occurs.

**Non-goals:** production migration, deployment, EC2 start, profile enablement, or
live canary. Relations: blocked by every prior node, explicitly `AO-MIGRATION`.
Closing this issue permits only the engineering authorization ruling, not
production action.
