# Continuous Operation Invariant and Resource Contract

This contract freezes the safety claim for the architecture approved on
2026-07-15. Values below are the initial autonomous BTC 5-minute profile. Every
value is owned by the typed TOML schema; the Rust runtime, deploy renderer,
systemd unit, and verification harness consume the same resolved configuration.
Missing, unknown, inconsistent, or unenforceable values prevent autonomous-mode
startup.

No implementation may weaken this contract to make a test pass. A different
value requires a reviewed configuration change, regenerated closed-form
accounting, and the same verification gates.

## Primary Invariant

For any length of event, outage, crash, and restart sequence:

1. Mutable resource usage is bounded by the tables below, independent of event
   count and uptime.
2. Recovery cardinality is a function only of unresolved risk and the finite
   active semantic state registry.
3. New risk cannot consume capacity reserved for reconciliation, exits,
   settlement, terminal evidence, or dependency recovery.
4. A timeout or missing event cannot erase or terminalize possible exposure.
5. Restart either reconstructs every unresolved item or keeps risk increase
   blocked while bounded reconciliation retries automatically.
6. S3 and raw capture are never recovery authorities.
7. A stable `(episode, canonical state)` produces at most one evidence record,
   even if raw inputs oscillate A→B→A without limit.
8. Loss of novelty truth produces no canonical evidence, saturates every possibly
   affected novelty/ordinal bit, and cannot make an old state eligible again.

## Invariant Matrix

| Invariant | Mechanism | Enforced where | Failure behavior | Verification |
|---|---|---|---|---|
| One Bolt recovery authority | One checksummed Capsule graph on two full replicas plus a 16-KiB digest-only witness; W votes only through a checksum-valid selector naming one fully synced exact digest/parent/device record; a same-digest two-device quorum containing a full replica selects authority; venue owns external facts | Startup quorum loader and every durable workflow transition | Missing/corrupt W selector abstains and no unselected child is inferred; A+B may repair W, but one full plus invalid W has no quorum; no compatibility/S3 fallback | Static authority fence plus every selector/record corruption and selection combination, sequential fail/return ordering, unbounded degraded commits, and no-S3/JSONL-read restarts |
| Recovery bytes are fixed | Per full replica: two `P` slots, `M` manifest/temp, fixed arena; witness: two records plus selector/temp; all three device projects are fixed | Fixed encoder, quorum publisher, and project quotas | Oversize successor is rejected; one-voter failure degrades and blocks entry | Maximum legal encoding, both replicas at `P-1/P/P+1`, witness edges, every crash artifact, and no payload temporary |
| Recovery cardinality ignores uptime | Fixed slots: 10 risks, 20 orders, 10 settlements, 13 episodes, 14 lifecycle bundles | Capsule allocator | Candidate is rejected; unresolved slots are never evicted | Model test reaches a fixed point under arbitrary repetitions |
| Every durable transition is registered | One authoritative at-most-512-row `ci/autonomous-transitions.toml` generates the Rust edge enum/table, sealed durable/effect capabilities, fault hooks, and crash matrix; stable ids are never reused. Until integration, an exact monotonically shrinking census may exempt only unchanged legacy-target callsites proved unreachable from autonomous entrypoints | Build generator, source fence, durable/effect wrappers, legacy target/link reachability proof, and CI bijection/census checks | Missing/orphan edge, row 513, autonomous-reachable or non-census direct durable syscall/effect, changed/new census callsite, absent fault hook/test/PR owner, or generated digest drift blocks build and autonomous activation; final integration requires the census empty | `transition_registry_bijection`, `durability_syscall_census`, `fault_hook_coverage`, `matrix_generated_from_registry`, `no_unregistered_external_effect`, `no_autonomous_census_reachability`, plus every generated crash test |
| Capacity precedes risk | One serialized candidate snapshot, fixed request, componentwise token acquisition, and `EntryPreparedNotAuthorized` commit precedes one final all-three `DispatchMayHaveStarted` commit containing the repeated predicates, finalized pre-dispatch block, and exact request/hash before the syscall | Shared admission service | Preparation is never send authority; stale/expired candidates become all-three `EntryAborted` without send. A crash after the final commit is conservatively ambiguous and retains every reservation | Race tests at every `capacity-1/capacity/capacity+1` boundary, every revalidated time/market/feed/health/capacity predicate, prepared-before-W repair, abort-release ordering, crash before/after the final commit and syscall, and command-sink assertion |
| Account capture is bounded and complete | Autonomous wallet is Bolt-exclusive; current-unresolved capture requests maxima plus one (`21` orders, `11` positions/claims), accepts no continuation, parses exact decimals/scaled integers, and retains every positive/dust/redeemable amount before execution-size filtering | Provider boundary, account-ownership preflight, and reconciliation | Extra item, cursor, unknown writer, `f64`/lossy conversion, invalid scale/range, or inconsistent current snapshot halts entry/repair; no account-wide terminal-history scan or truncation | Inject 20/21 orders, 10/11 claims, pagination, delayed updates, concurrent external writer, positive sub-threshold dust, decimal scale/range edges, and forbidden float/filter paths |
| Prepared-order resolution is exact and bounded | The exact signed hash is resolved only by a source-fenced `ProviderTerminalCertificate`: `Filled` carries a complete sorted unique set of at most 64 canonical 32-byte transaction hashes; each sequential receipt is at most 2,097,152 B/4,096 logs. `PermanentlyTombstonedNoEffect` is a linearizable, restart/rollback-durable hash tombstone ordered after all submit/delay/retry/match/duplicate/preapproval work. Finalized V2 status, receipts/indexed `OrderFilled` logs, and exact post-state verify the certificate | AO-NT provider contract, Polygon RPC boundary, order state machine, generated provider-capability gate, and fixed order slots | 404/absence, cancel/not-canceled, elapsed time, wire expiration, heartbeat, ordinary order/trade status, or quiet chain never releases or replays. Item 65, oversized receipt/log set, missing/duplicate/conflicting hashes, or a provider that cannot guarantee completeness within the cap remains Unknown and disables entry. Without the permanent tombstone contract the autonomous profile is mechanically incapable of entry; zero allowance is temporary protection only | Provider linearizability/restart/rollback/duplicate/preapproval fixtures; tombstone-before-submit and submit-before-tombstone races; response loss; exact route truth tables; hash-set `63/64/65`; receipt/log cap edges; receipt reorg/corruption; exact maker/taker/fee logs; repeated ambiguity without slot attrition; no account-history or uptime-sized log scan; current V2 negative capability fixture must keep activation blocked |
| Redemption is deterministic and bounded | A generated provider manifest/TOML bind the exact collateral-adapter and relayer/Safe contracts. One 16,384-B account-global `SafeNonceLane` permits only one current nonce owner across all conditions and reserves two 4,096-B bodies: the original redemption and one deterministic same-nonce fence calling the source-fenced Safe `nonce()` getter with zero value. The settlement slot retains exact balances, identities, relayer ids, Safe hashes, receipts/logs, and post-state | Rust-native settlement worker, SSM-only relayer signer, generated provider boundary, Capsule settlement state machine, and Safe nonce lane | `NEW`/`EXECUTED`/`MINED` and `FAILED`/`INVALID` never release alone. Byte-identical retry or the committed same-nonce fence continues. Redemption winning proves success; fence winning plus finalized nonce advance and unchanged claim/post-balance proves permanent no effect. Any other nonce consumer or relayer inability to accept an explicit competing nonce blocks/integrity-halts; S3 is irrelevant | Exact ABI/manifest fixtures; two-condition same-nonce race; original/fence ordering; response loss at both syscalls; every relayer state; competing-nonce conformance; receipt reorg/log corruption; unexplained nonce consumer; exact post-balance/dust; restart at every lane phase; maximum two-body encoding |
| Exit and settlement capacity is protected | Per-risk evidence partition, 64 compact pending receipts in the authoritative Capsule, four future retry owners, and recovery-only memory/task/FD/order/settlement tokens are acquired before entry | Token classes that ordinary paths cannot borrow | Entries, diagnostics, and archive PUTs stop first; with a valid Capsule quorum, risk reduction remains scheduled even when both non-authoritative evidence arenas are unavailable. Only loss of the current Capsule authority prevents a new effect | Exhaust every ordinary class and fail both arenas while retaining Capsule quorum, then close and settle all ten risks concurrently and materialize receipts after repair |
| Evidence is once per semantic state | Fixed receipt `Unseen -> PendingArena` and novelty commit together; only the dedicated receipt materializer can then prepare/write/archive it | Capsule evidence transaction | Repeated raw state is a no-op; pending materialization never reevaluates volatile data | Indefinite-in-principle receipt state-machine test, crash injection at every edge, and large A→B→A tests |
| Lost novelty cannot recur | Payload-loss quarantine durably sets `UnknownIntegrityEvidenceFence`, disables all canonical evidence, saturates every risk/market family and risk ordinal through trusted current+next windows without enumerating lost episode ids, and permanently saturates every state bit in the lost/current system episode; masks only OR | Exceptional recovery, migration ambiguity handling, novelty encoder, and trusted frontier | Without trusted time the disable stays open-ended; repair cannot unsaturate old bits; only genuinely new risk/market episodes in the first exact-slug market strictly beyond the installed two-window fence may receive fresh masks; the old system episode never does | Emit each state class, lose current payload or import unidentifiable ambiguity, hide facts, remove/restore trusted time, repair/restart across wrap, and prove no system recurrence plus no risk/market evidence before fresh discovery beyond the fence |
| Canonical-state domains are closed | One TOML registry with exact risk 64, market 256, and system 64 family/id allocations; unassigned ids cannot emit | Typed registry generation, config validation, and every evidence callsite | Unknown/duplicate/out-of-family state prevents startup or compilation; no fallback diagnostic state | Source census plus generated exhaustive transition-to-state map and family-bound tests |
| Evidence episode identity is stable and non-temporal | `EvidenceEpisodeId` contains logical strategy/target/venue plus only stable venue semantics `(Gamma id, condition id, question id, neg-risk mode, ordered exactly-two(outcome index, normalized outcome, CLOB token id))`; a risk adds one non-reusable ordinal `0..9`. Slug, serial/window, open/close, every timestamp, price, feed flag, config/schema/deploy value, retry, diagnostic, and order id are excluded | Typed evidence API and generated identity encoder | Temporal/raw churn cannot allocate, rekey, roll, or clear an episode/mask; a genuinely new condition/market id needs the reviewed next-market transition and durable empty barrier | Change slug/window/timestamps and oscillate every volatile value after binding; the episode/masks remain byte-identical. Change a stable venue field in-place and prove the lane blocks without resetting; transition to a new condition id and prove exactly one new episode |
| Gamma discovery binding is exact but not evidence identity | A separate durable `GammaMarketBinding` holds Gamma/condition/question ids, exact discovery slug, trusted open/close window, neg-risk mode, and ordered exactly-two outcomes/token ids; complete hydration occurs before bundle/episode creation | Gamma hydrator and lifecycle gate | Missing token ids stay in bounded `DiscoveryHydrating`; later binding mutation blocks the lane/serial but cannot change the already bound `EvidenceEpisodeId` or novelty | Zero/one/two/wrong/incomplete results; mutation of each field/order/count; explicit slug/window/timestamp-only mutation with invariant episode id and masks |
| Active novelty cannot be evicted | Thirteen fixed episode slots remain until terminal; one lane has adjacent normal retirement plus a constant-work `FrontierRebased` transition after bounded account capture, at most two exact-slug current/next Gamma queries, complete hydration, expiry rejection, and a non-replenishing ten-bit risk mask keyed only by `EvidenceEpisodeId` | Capsule lifecycle and discovery gate | Old/ambiguous identity is rejected before episode construction; mutation after binding blocks the lane while preserving the existing id/masks; long downtime creates no backlog/episodes and saturates current masks; used ordinals never reopen | Reintroduce old identities; churn slug/window/time and all volatile inputs; mutate stable binding fields; zero/one/two/wrong/incomplete Gamma results; gaps, arbitrary downtime, multi-wrap rebase, and `MAX->0` adjacency across 100,000 rollovers; no duplicate/reactivation |
| Evidence backlog is fixed | One logical arena mirrored as two preallocated 960-record physical replicas, 32,768 bytes each; no general runtime producer | Receipt-index-to-offset mapping, quorum receipt transition, and file length guards | No append or spill path; one arena failure blocks entry and repairs; archive health blocks new risk | Fill all partitions, fail/repair either replica, and inspect exact lengths/cardinality |
| Volatile churn cannot reset reserves | Physically disjoint risk, market, and system offsets; no general runtime range | Typed fixed-offset router | One class cannot borrow another | Independently saturate each class while outer inputs oscillate |
| S3 is historical only | Capsule contains all unresolved workflow state; uploader accepts immutable frames only | Archive interface and startup source fence | Outage fills local cap and stops entries; restart and exits continue locally | Restart in every required risk state with S3 absent and backlog full |
| S3 upload is idempotent | One prepared deterministic object, one finite ring/legacy key position, conditional PUT, SHA-256 and length verification | Rust archive worker | Unknown result retains the same local object; wrong pending content is delete/list-absent/recreated only under verified exclusive authority and is never freed first | Unavailable, slow, duplicated, partial, response-lost, pre-ack corruption repair, and post-ack historical-loss tests |
| S3 retry state is fixed | One in-place retry episode, one timer, capped delay and saturated index | Lifecycle scheduler | No task, log, or record per attempt | Run an arbitrary number of failures and compare invariant snapshots |
| S3 retention is bounded | Dedicated never-versioned bucket, exclusive prefix, a 365-slot market ring with durable ownership/delete/list/empty barriers, one protected legacy slot, and at most 12 objects per market. Before first ownership, one fixed cursor proves all 366 prefixes empty with `ListObjectsV2(MaxKeys=1)` | Retention worker, bucket-state verifier, IAM verifier, and Capsule cohort cursor | Unexpected pre-existing key, any nonempty versioning status, or unverifiable empty barrier integrity-halts/stops PUT; legacy cannot prune before all-object revalidation/local deletion authorization; local backpressure stops entries | Initial 366-prefix census including preseeded key; every cohort/retry transition; ring-index/day-tag wrap; arbitrary downtime; 366-day clock; legacy upload beyond 365 days; deletion delay; version-state drift; one request in flight |
| Capsule publication is unambiguous | The same child commits with two distinct device votes including one full replica; W counts only through its synced selector-selected record; risk increase requires all three voters and both arenas identical | Quorum atomic writer | Startup follows quorum digest/parent/selector proof, never generation/device priority; one failed voter permits only non-risk-increasing progress | Crash every voter-record/selector ordering; prove no two current digests have quorum, unselected W children abstain, and stale media cannot authorize an effect |
| Lost/corrupt manifest is conservative | Direct parent digest plus the schema-generated field/workflow join table; every reusable identity passes through a durable empty barrier | Startup repair | Child-only `EntryPreparedNotAuthorized` remains source-fenced as never sent and may only receive a fresh all-three final commit or abort; `DispatchMayHaveStarted` and every incomparable may-have-started phase remain query-only with full reservation. A negative exact-id query never authorizes replay or release | Exhaustive join/provider-effect tests including A+B prepared before W, prepared final-commit/abort without query, perpetual negative/unknown exact-id queries after may-have-started, crash on both sides of the final commit/syscall, byte mismatch rejection, and every maximum |
| Corrupt Capsule never implies flat | A quorum-selected full copy repairs stale voters; without quorum only the closed direct-parent join is legal. Dual-full-media loss/decommissioning is catastrophic. One 8,192-B/two-inode selector pair is adopted by `SELECTOR-INIT-001..004`; all later `RELEASE-SWITCH-*`, `DEV-EPOCH-*`, and `ACTIVATE-*` mutations use one parent-directory lock, current inode/device plus full-record compare-and-swap, and exact pre/post exchange proof. Release/device changes force `CapsuleDisabled` and clear stale autonomy authorization. `DEV-EPOCH-001..007` reinstalls/verifies the boot-volatile kernel old-device denylist before voter reads; step 006 permits capture only and step 007 alone enables replica A. The replacement epoch binds the sentinel `CatastrophicBootstrapCertificate`; publication is A then B then selected W after unchanged recapture | Active-release selector in system-mutable project, sole selector mutator/lock, immutable release admission record, prestart device allowlist, deploy verifier, startup catastrophic publisher, and quarantine reducer | Old selected release keeps bootstrap ineligible; a fully durable new selector makes old lineage ineligible but remains entry-disabled. Every restart closes the voter-read gate until the kernel fence is rebuilt. Missing/corrupt/mixed/rollback selector, stale expected record or authorization, undrained process, or voter open before the verified fence halts without reading recovery bytes. Before same-certificate A+B+W: no effect/evidence | Crash at every selector-init/release-switch/`DEV-EPOCH-001..007`/activation boundary; concurrently interleave all selector writers and prove one CAS winner; replay stale authorization; verify exact inode/device/digest mappings; fail same-filesystem/exchange support; restart/return stale media; prove zero voter opens before step 005 and zero A publication before step 007; then crash every A/B/W edge and mutate every certificate field |
| Migration has one cutover | Old runtime first disables entry and proves authoritatively flat while it still manages exits/settlement. The migration identity then enforces single-link regular files, exact directory topology, no writable FD/MAP_SHARED, immutable, exclusive-read sealed inputs; direct I/O streams at most 16,383 source paths using four root FDs plus one source FD. One 4-KiB blocker is created/synced before quota sealing, so the fixed 16,384-descriptor inventory includes it without later allocation | Old runtime flat certificate, systemd migration service, kernel permissions/immutable/mapping fence, migrator, release fence, and quorum publisher | Dependency failure before flat leaves old runtime active; afterward the sealed source remains authority. A crash accepts exactly the pre/post exchange inode mapping, discards bounded memory, and rescans; activation requires both full manifests, selected witness, and both arenas identical | Nonzero/unknown risk before stop; hardlink alias; directory topology drift; closed-FD writable mmap; warm cache; blocker/quota/path/record caps; five-FD proof; crash at every blocker/scan/sort/upload/bootstrap edge; no scratch/progress authority |
| Imported evidence has one owner | Registered recovery/evidence JSONL frames enter the `N*40` sort and a length-preserving classified binary S3 history stream; raw bytes never egress. Allowlisted raw-only families archive unchanged; unapproved families are permanent local quarantine. Malformed/conflicting frames use exactly `HistoricalOnly`, exact permanently terminal `TerminalAssociationOnly`, or blocking `RecoveryBearingUnsafe`; migration begins only from an independent flat certificate | Frozen classifier, JSONL decoder/classified encoder, descriptor sort/join, novelty encoder, arena router, and egress/quarantine deletion fence | Historical classes quarantine and saturate the whole episode/unknown-integrity fence. Any ambiguity about may-have-started, identity, amount, account, permanent terminality, current exposure/settlement/capacity, or flat truth blocks; no venue aggregate becomes authority | Permutations/A→B→A, three conflict classes, raw-leak and path matrix, `N`/frame/path edges, digest collision, HMAC rotation, and every egress deletion crash while quarantine remains exact |
| Legacy input is bounded | Each registered JSONL frame <=2,097,152 B; source `S=2,151,809,024 B`; `F_total=16,384` inventory paths include one blocker and at most `F_source=16,383` <=512-B source paths. `S_egress<=S`, `F_egress<=F_source`, and `L_actual=S_egress+640F_egress<=2,162,294,144 B`; quarantine remains within the fixed local legacy claim | Migrator inventory/classifier/archive encoder | Any frame/source/path/metadata/representation overflow blocks cutover; recovery-bearing ambiguity blocks; no unapproved raw byte can egress or enter deletion | Incident fixture, every cap `-1/cap/+1`, blocker reservation/exchange, raw-leak scan, torn/corrupt input, path metadata/reopen/continuation edges, quarantine survival, and conflict classes |
| Legacy record count `N` | 1,048,576 | TOML maximum; incident input has 272 records | Inventory/classifier and 40-B descriptor arena | `N+1` blocks cutover; no descriptor spill |
| Local mutable disk and inodes are bounded | Two full-recovery projects, one witness project, reports, journal, releases, legacy, and system-mutable have exact byte/inode claims; migration creates no scratch; root is read-only and `/tmp`/`/run` are bounded tmpfs | Prestart, periodic health, deploy retention, project quotas, mount sandbox, and admission | Entry/publish blocks before any future-write byte or inode floor is crossed; unknown writable/scratch path fails startup | Quota/inode/full/read-only/fsync tests, three recovery device identities, all project-colocation floor combinations, path census, tmpfs saturation, and maximum-state inventory |
| Memory is bounded | Main operational classes total 3.5 GiB with a 256-MiB unallocatable guard under 3.75-GiB `MemoryMax`; archive operates below 192 MiB with a 64-MiB guard. Generated closed-form `N_main=max(N_live,N_migration)<=512 MiB` includes native-thread guard/VMA/page-table metadata (resident stack pages excluded), ELF/DSOs, other VMAs/page tables, allocator arenas/metadata, recovery/config page cache, process-attributed kernel objects, and main-cgroup retained socket rows; generated `K_host<=640 MiB` itemizes pinned base, per-CPU, per-device, filesystem, cgroup, global-network, and only root/unmanaged retained-socket-state terms. The exact claims total 8,053,063,680 B and the accepted observed `MemTotal` interval is `[8,053,063,680, 8,589,934,592]` | Allocator/queue gates, full pre-open ballast reservation, cgroups/slices, generated `N_main`/`K_host` ledgers, source fences, host-reserve service, and systemd | Ordinary work cannot obtain recovery pages; unknown claim or coefficient cannot replace ballast; worker failure retains evidence; reserve/ledger drift blocks entry while 1.5-GiB `MemoryMin` protects main recovery/overhead | Generate and sum every resolved row; saturate/measure every class; maximum upload/delete/list/IMDS and migration inside operational ceilings; full socket reservation before open; guard crossing; reserve recharge; undersized host; CPU/device count edges; and ten-risk closure |
| Async futures and native threads are independently bounded | A 512-entry async-future registry partitions 384 ordinary/128 recovery futures; a separate 128-entry native-thread registry, full stack/guard claim per thread, and effective `TasksMax=128` bound OS threads; connection generations close/join | Async spawn wrapper, native-thread wrapper, provider boundary, ballast, and systemd | Ordinary futures coalesce; 128 recovery futures remain; a 129th native thread is denied regardless of unused future capacity | Future and native-thread cap+1 tests, every stack reservation before create, spawn/reconnect storm, and registry/cgroup/effective-unit inspection |
| File descriptors are bounded | Budgeted acquisition and `LimitNOFILE` | File/socket wrappers, source fence, systemd | New feeds/archive work wait; 512 recovery descriptors remain | `/proc/self/fd` saturation, restart, reconciliation, exit and settlement test |
| Logs are bounded | Dedicated journal target 512 MiB inside a hard 576 MiB filesystem project quota; no autonomous file logger; transition-deduplicated retry logs | Generated journald/unit config, project-quota verifier, and log adapter | Old journal data rotates/drops; logging failure never blocks reductions | Log storm through rotation/active-file overlap and mechanical project-quota inspection |
| Secrets and signed provider bytes never enter observability | CLOB and relayer signed request/Safe bodies, signatures, authorization headers, every SSM credential value, and every raw provider success/error/request buffer are non-loggable types; only fixed redacted ids, lengths, outcome classes, and digests can cross log/evidence/alert traits | `AO-NT` and `AO-REDEEM` provider adapters, typed observability boundary, source fence, and sink capture | An attempted formatter/serialization path fails build or the activation source fence; runtime errors collapse to bounded redacted metadata and never persist raw bytes | Compile-fail/source-census tests plus sentinel-valued successful, failed, malformed, oversize, and response-lost fixtures captured across journal, evidence, report, and alert sinks; assert no secret/request/raw-response substring and exact allowed fields |
| Network inputs and lifetime are bounded | Wire-frame, decoded-message, request/response, item, byte, and concurrency caps plus generated `NetworkFootprint` and `NetworkLifetimeFootprint`; 18 HTTP-owner rows/17 populated, 19 origins/18 populated, 18 DNS/TLS rows/17 populated, 34 live sockets/30 protected. Per-owner dial buckets and stable-reset intervals bound TIME_WAIT, FIN/orphan, conntrack, ephemeral-port use, and their BTF-priced retained state | Provider decoder before parsing, bounded channels, every connect/client/spawn/raw-socket wrapper, and signed-AMI sysctl/socket-option verifier | Unknown owner/coefficient, cap+1 open, early bucket reset, or projected retained-state/port exhaustion blocks before dial; market data coalesces; delta loss invalidates/resnapshots; critical overflow blocks entry and reconciles | Oversize/saturation for every lane; add missing alert/RPC/relayer rows from audited 14/15/14 to intended 17/18/17; cap+1 owner/origin/DNS/live-socket/dial tests; TIME_WAIT/FIN/orphan/conntrack retention storms; ephemeral-port reserve; overlapping redial rejection; `/proc`, BTF, and cgroup census |
| Retry/pending work is bounded | A fixed 96-owner table: 64 protected recovery owners and 32 ordinary owners; each candidate risk acquires four future recovery owners before entry | Retry ledger and Capsule slots | Backoff saturates; no terminal give-up; ordinary owners cannot consume recovery ownership | Millions of retry transitions, maximum simultaneous closure, and every partition boundary with unchanged cardinality |
| Rollover cannot leak | Fourteen durable lifecycle bundles/14 Polymarket asset-id wire members shared by book/trade consumers; the pinned provider can write per-asset unsubscribe but supplies no server acknowledgement, so the autonomous source fence makes that call unreachable and every desired-set change performs whole-generation close/join/recreate with one physical socket per generation owner. `Absent -> Requested -> Observed` requires a current-generation full book snapshot or source-fenced sequence-complete baseline; expiry transfer precedes WS construction; downtime rebase is constant work; caches retire by refcount. Every provider operation is cancellation-safe and TOML-time-bounded | Lifecycle supervisor and pinned NT/Gamma provider boundaries | Subscribe write, delta, or trade success is not observation; delta-before-baseline invalidates/resnapshots; timeout/mutation replaces the entire generation; any autonomous per-asset unsubscribe attempt fails closed; a task that misses `market_generation_join_deadline_ms` keeps entry blocked and causes bounded self-termination/systemd restart; overlapping generations and expired assets are rejected before wire; REST remains order truth | 100,000 rollovers; every desired-set change; exactly zero per-asset unsubscribe calls; delta/trade-before-snapshot; source-fenced complete baseline; stuck read at every edge followed by deadline/restart; late old-generation message; binding mutation/identity churn; arbitrary downtime/gaps/wrap; task/FD/socket/cache census; stable 14/64 caps |
| External outages cannot strand risk | Orders, positions, exits, and settlements remain Capsule-owned; any current two-vote quorum durably prepares reductions. AO-NT provides bounded current capture and exact prepare-query-replay; the Rust-native relayer/Safe path retains one deterministic redemption identity and current-collateral post-state proof | Recovery/lifecycle/settlement supervisors, quorum storage, generated provider manifest, and pinned provider boundary | New risk stops on any required-health loss; existing reduction/redemption resumes when only its specific dependency and a current full-replica quorum return. S3, archive, alert, or market-data outages cannot consume the reserved exit/settlement lane or change authority | Toggle venue/network/RPC/relayer/S3/alert and each voter in every phase; sequentially fail/return A/B/W; restart with each unresolved workflow; duplicate/partial relayer outcomes; remove current payload; inject prior unknown orders; prove automatic resumption without operator repair |
| Process recovery is automatic and bounded | Generated unit uses `Restart=always`, TOML-owned constant restart delay, `StartLimitIntervalSec=0`, bounded startup timeout, and journal rate limit | systemd renderer/effective-unit verifier | Crashes retry forever without accumulating timer/history or entering operator-reset state; journal quota contains the storm | Thousands of crash/restart cycles, host reboot, dependency restoration, effective-unit and journal-allocation inspection |
| Alerts are bounded and non-authoritative | One latest-state slot per fixed system-health id, one prepared Rust-native message, one in-flight send, and one saturated retry owner | Capsule health registry and alert worker | Transport outage never grows a queue or blocks reduction; recovery sends the latest active/clear state automatically | Oscillate every alert state with transport absent/slow/duplicate and prove fixed cardinality plus automatic recovery |
| Autonomous recovery needs no normal operator | Admission is a conjunction of self-clearing health predicates for all normal modeled failures, including automatic replica repair | Central health state machine | False predicate blocks entries; all-true state reopens automatically; destruction/corruption of both independent replicas remains a rare authenticated incident | Repeated failure/restoration tests including each replica and full S3 backlog, plus proof normal paths never request repair |
| One config source owns every bound | Typed TOML renders runtime objects and systemd/journald settings; effective settings are compared at startup | Config loader, deploy renderer, source fence | Missing, zero, mismatched, or unknown setting prevents startup | Config omission/unknown/mismatch tests and generated-unit equivalence |
| Intermediate releases make no autonomy claim | Every component lands disabled; the final integration head contains one production-capable profile and one stopped-service migration entrypoint but does not invoke either. Enablement additionally requires all provider capability gates, the engineering `AUTHORIZED` ruling, and separate operator approval | Config validation, provider-capability manifest, release renderer, and PR contract | Main can be fail-closed or supervised but cannot advertise autonomous readiness; a false capability gate makes profile construction fail | Per-PR profile-start assertion, final disabled-by-default invocation test, negative provider capability fixtures, and proof that no PR/deploy/test path performs production cutover |

## Resource-Bound Table

All byte values are exact maxima, not observed averages.

| Resource | Bound | Function of | Enforcement point | Exhaustion behavior |
|---|---:|---|---|---|
| Capsule payload slot `P` | 1,048,576 B | Fixed section layout | Encoder before write | Reject successor; block new risk |
| Capsule manifest `M` | 4,096 B | Slot id, schema, length, digest | Fixed manifest encoder | Reject publication |
| Generated transition registry | At most 512 total rows including retired; at most 512 active descriptors × 64 B = 32,768 B read-only; at most 8 durable ops/active row | TOML header freezes total rows/descriptor bytes/op count; retired ids consume row capacity but emit no descriptor; no per-event allocation | Build generator, closed-enum registry/source-owner/callsite bijection checks, temporary unreachable legacy-callsite census, and release ELF accounting in `N_main` | Row 513, ninth durable op without an intervening recoverable edge, a missing/orphan edge/source owner, an autonomous-reachable/non-census direct durable syscall/effect, changed/new census callsite, or generated-digest drift blocks build/activation |
| Capsule crash peak per replica / pair | 2,105,344 B / 4,210,688 B | Per replica `2P+2M`; successor writes directly into the inactive payload and only the manifest has a temporary | Preallocation, file inventory, mirrored writer, and offset guard | Write cannot cross peak; a payload temporary is forbidden |
| Commit witness | 16,384 B / 8 inodes hard inventory; 65,536-B project ceiling | Two 4-KiB records + 4-KiB selector + 4-KiB selector temporary on a third device | Quorum writer, selector checksum/sync verifier, device-id verifier, and project quota | Missing/corrupt selector abstains; an unselected child never votes; A+B may repair W, while one full plus invalid W has no quorum |
| Active risks `N` | 10 | Autonomous profile capacity | Admission ledger | Candidate rejected |
| Risk admissions per market | 10, non-replenishing | One ordinal bit `0..9` per canonical market | Capsule market ledger before candidate allocation | Eleventh risk is rejected even if earlier risks are terminal |
| Unresolved orders | 20 | `2N` prepared/unknown normal entry/exit owners | Order slot allocator | Candidate rejected; no undurable overflow action exists |
| Fill-certificate transaction hashes | 64 per order / 1,280 across 20 order slots / 40,960 B encoded | TOML `max_fill_transaction_hashes=64`; each canonical hash is exactly 32 B and each 12,288-B order slot reserves 2,048 B for its sorted unique set | Provider capability manifest, certificate decoder before allocation, and order-slot encoder | Item 65, duplicate/conflicting hash, missing completeness proof, or provider inability to guarantee the cap leaves the order Unknown and keeps autonomous entry disabled; no history scan substitutes |
| Fill receipt verification | At most 64 sequential receipt requests per order; one in flight; response <=2,097,152 B and <=4,096 log items | One request for each retained exact hash; TOML HTTP/log caps and provider batch/receipt conformance | Polygon RPC decoder, protected HTTP lane, exact exchange/log filter | Oversize/item overflow, reorg, wrong exchange, duplicate log coordinate, or incomplete response leaves the order Unknown without allocating another cursor or request |
| Settlements | 10 | `N` | Settlement allocator | Candidate rejected before risk starts |
| Active episodes | 13 | `N+current+next+system` | Episode allocator | No new episode/risk |
| Autonomous market lanes / horizon | 1 lane / 2 windows | BTC profile and contiguous retirement proof | Config validation and lifecycle registry | Wider profile is rejected pending a new proof |
| Polymarket lifecycle bundles | 14 | `N + horizon(2) * legs(2)` instrument/role owners, each owning at most one asset id | Lifecycle registry | New asset/risk waits |
| Polymarket wire asset ids | 14 | One asset-id wire member per lifecycle bundle; book/trade consumers share it | Generation-scoped `Absent -> Requested -> Observed` registry | Transport write or pre-baseline delta creates no observation; entry waits for current-generation full-snapshot/sequence-complete `Observed` plus a fresh complete book |
| Global subscription wire members | 64 | All configured providers and wire identities | Actual registry and config validation | Startup/admission fails closed |
| Instrument cache entries | 64 | All configured static and dynamic instruments | Refcounted cache | New instrument blocked; active leases retained |
| Evidence records `A` | 960 | `10*64 + 256 + 64` | Fixed receipt/state offsets | New history/risk blocks by partition |
| Evidence record `R` | 32,768 B | Bounded binary envelope including padding | Encoder and offset check | Oversize state rejected before commit |
| Evidence arena per replica / pair | 31,457,280 B / 62,914,560 B | `A*R` on each recovery device; both are copies of one logical arena | Preallocated fixed files and mirrored receipt materializer | Cannot grow; one failed replica blocks entry and is repaired from the survivor |
| Risk evidence reserve | 640 records / 20,971,520 B | `N*64*R` | Per-risk ownership | Ordinary paths cannot borrow |
| Materialized market reserve | 256 records / 8,388,608 B | One current market registry; next remains in Capsule receipts until durable empty-barrier transfer | Market owner | Next-market risk blocks; existing-risk partitions remain |
| System/recovery reserve | 64 records / 2,097,152 B | Fixed semantic registry | Recovery owner | Non-system record denied |
| Prepared S3 object | 256 records / 8,392,704 B | `256*R + 4,096` envelope | Capsule selection + direct worker buffer | No second object starts |
| Legacy decision-evidence input | 2,097,152 B | Configured JSONL ceiling | Streaming reader | Cutover blocks/reconciles |
| Legacy kill-switch state | 65,536 B | Current bounded store ceiling | Streaming reader | Cutover blocks/reconciles |
| Legacy manual-recovery audit | 2,097,152 B | Configured audit ceiling | Streaming reader | Cutover blocks/reconciles |
| Optional legacy basket state | 65,536 B | Quarantine ceiling | Streaming reader | Incompatible profile/cutover blocks |
| Aggregate recovery migration input | 4,325,376 B | Sum of four fixed inputs | Migration inventory | No overflow or truncation path |
| Total sealed legacy scan input `S` | 2,151,809,024 B | Raw/history 2,147,483,648 + recovery inputs 4,325,376 | Read-only inventory and streaming scanner | Any extra byte/path blocks cutover |
| Legacy migration clean work | At most `3S+4A*F_source+2AN=15,313,780,736` aligned source bytes and `N+2F_source=1,081,342` source-data opens per clean generation, with `A=4,096`, `F_source=16,383`, `N=1,048,576` | One directory traversal; inventory/classification, one semantic reread per descriptor including collision groups, and classified/exact-byte egress regeneration; four fixed root FDs plus one data FD; no path re-enumeration or extra collision pass | Deterministic no-scratch migrator under sealed-source/direct-I/O fence | Crash repeats the same bounded generation; no progress file, merge run, spill, or derived local authority exists |
| Migration path inventory | 1,048,576 B / 16,384 descriptors plus 10,485,760 B source metadata | 64 B per path stores type/length/SHA-256/virtual range/metadata index; 640-B row stores root index, complete <=512-B normalized relative path, and class. Exactly one row is the blocker and at most 16,383 are sources | Fixed arrays, virtual-range binary search, `openat2` beneath four root FDs | A second blocker, source path `16,384`, total path `16,385`, 513-B path, alias/symlink/type/content mutation, bad range/index, or array overflow blocks and rescans |
| Migration semantic descriptors | 41,943,040 B / 1,048,576 records | Exactly 40 B per registered recovery/evidence JSONL record; allowlisted raw-only and quarantined raw/catalog/cache families never enter the semantic sort | Fixed in-memory sort plus full-key reread on digest collision | `N+1`, unregistered semantic input, collision ambiguity, or spill request blocks cutover |
| Migration workspace | 134,217,728 B in the main ordinary pool while live runtime is stopped | 33,554,432 aligned direct-I/O input + 41,943,040 semantic descriptors + 1,048,576 path inventory + 8,392,704 object buffer + 33,554,432 Feather/decoder + 10,485,760 source reopen/egress metadata + 5,238,784 join/key/slack | Migration allocator, five-source-FD/direct-I/O rule, and live-runtime exclusion | Allocation/cap drift or buffered fallback blocks; source payload cache must be zero after fadvise/mincore; metadata cache is separately charged; no spill/dirty scratch/second workspace |
| Legacy filesystem metadata memory | 134,217,728 B combined maximum | Allocated sealed-directory blocks plus signed-kernel inode/dentry/xattr coefficients for 16,384 total inodes including the pre-created blocker, tagged by effective main-cgroup or root/unmanaged charge owner | Migration preflight, `N_migration`/`K_host` generated rows, cgroup/slab census | Overflow or unknown/duplicate owner blocks before stop; max-depth/name tree and blocker remain inside the fixed cap |
| Legacy frozen object table | 258 × 40 = 10,320 B | Per fixed key: payload length + SHA-256, inside fixed migration section | Capsule encoder on both full replicas | Oversize/count mismatch blocks bootstrap; HMAC rotation cannot alter frozen bytes |
| Runtime recovery set | 67,141,632 B | Two full replicas × (`2P+2M+A`) + 16,384-B witness | Three recovery project quotas and file inventory | One voter may fail; fewer than two matching votes or no current full payload blocks effects |
| Worst recovery set including retained recovery legacy | 71,467,008 B | Runtime voters + 4,325,376 B sealed migration inputs | Migration/runtime inventory | No marker publication or new risk |
| Configured recovery ceiling | 157,351,936 B aggregate / 78,643,200 B per full replica + 65,536 B witness | 150 MiB full-replica projects plus fixed witness project | Project quotas + three distinct device identities + inventory | New risk/cutover blocked; voters cannot share a device |
| Recovery/data-filesystem free-space floor | 10,737,418,240 B | 10 GiB after every remaining project claim when any recovery/data class is present | Per-device prestart, periodic, migration, and admission checks | New risk/cutover blocked |
| Root/log-filesystem free-space floor | 2,147,483,648 B per distinct root/log-only device | 2 GiB after every remaining project claim only when no recovery/data class shares the device | Per-device host/deploy checks | Publish/entry blocked; running release remains |
| Per-device byte/inode predicate | `f_bavail(d)-Σremaining_bytes(i)>=max(applicable_class_byte_floors(d))` and `f_favail(d)-Σremaining_inodes(i)>=65,536` | Every registered project sharing `d`, summed once; byte floor is 10 GiB if any recovery/data class is present, otherwise 2 GiB; inode floor applies once; at most four devices | Generated device/project ledger and quota verifier | No admission/publication before every permitted future write exists |
| Mutable filesystem/device registry | 4 devices / 7 project classes + 2 tmpfs classes | Full recovery, witness, reports, journal, releases, legacy, and system-mutable; `/tmp` and `/run` tmpfs | Closed TOML registry, device-id resolver, read-only root, and mount sandbox | Fifth device, unknown writable/scratch path, bind drift, or uncharged project blocks startup/cutover |
| Cold recovery-device capacity | 10,816,061,440 B / 65,552 inodes each | 10-GiB/65,536 floor + one 75-MiB/16-inode replica project | Prestart on two distinct device ids | Startup/admission fails closed |
| Cold migration data capacity | 13,438,550,016 B / 81,924 inodes | 10-GiB/65,536 floor + legacy 2,684,354,560/16,384 + reports 16,777,216/4; migration creates no local derived file | Migration preflight | Cutover fails closed |
| Cold data capacity after migration | 10,754,195,456 B / 65,540 inodes | 10-GiB/65,536 floor + reports 16,777,216/4 | Runtime preflight | Startup/admission fails closed |
| Cold root/log+witness capacity | 5,435,883,520 B / 180,488 inodes | 2-GiB/65,536 floor + witness 65,536/8 + journal 603,979,776/256 + releases 1,610,612,736/49,152 + system 1,073,741,824/65,536 | Host/deploy preflight | Startup/publish fails closed |
| Legacy logical source | 2,151,809,024 B | 2,147,483,648 raw/capture/catalog/NT state/cache + 4,325,376 recovery inputs | Rust streaming classifier | Cutover refuses; no truncation/capture |
| Legacy allocated-block/inode ceiling | 2,684,354,560 B / 16,384 inodes | Sources may consume at most 2,684,350,464 B/16,383 inodes; the pre-created persistent blocker reserves exactly 4,096 B/1 inode. Includes all allocation rounding/xattrs/cache/catalog/capture/recovery inputs | Per-filesystem project quotas, `statx.stx_blocks*512`, quota `curspace`, sealed inventory | Cutover refuses before blocker creation or seal; no post-seal allocation is required |
| Recovery project inodes | 16 per full replica + 8 witness | Full: payloads, manifest/temp, arena, lock/probe/directory; witness: two records, selector/temp and allowance | Project quota and exact inventory | Extra inode blocks startup/entry |
| Legacy archive object | 8,392,704 B | At most 8,388,608 B of the #883 exact-byte-egress-allowlisted length-preserving source stream plus one 4,096-B envelope; a path continuation may cross an object only through the envelope's fixed continuation metadata | Single Rust migration uploader and one fixed object buffer | No second object/staging buffer starts; partial/unknown PUT retains the same bytes and key; unapproved families never enter the buffer |
| Legacy archive remote output | At most 2,163,350,912 B / 258 objects | `S_egress<=S`, `F_egress<=16,383`; length-preserving classified JSONL plus exact-byte raw payload `L_actual=S_egress+640F_egress<=2,162,294,144`, with `object_count=ceil(L_actual/8,388,608)` and 4,096-B envelopes. Unapproved bytes and blocker never egress | Deterministic registry/inventory/batcher and Capsule cursor `0..258`; unused suffix entries are `Empty` | Egress sources remain bounded locally until exact acknowledgement/revalidation; no local output copy exists and unapproved/raw-JSONL bytes never egress |
| Journal configured target | 536,870,912 B | 512 MiB | journald namespace | Rotation starts |
| Journal hard allocation | 603,979,776 B | 576 MiB including active-file/rotation overlap | Filesystem project quota | Further journal writes fail/drop; reductions continue |
| Journal inode ceiling | 256 | Namespace directory, active files, rotations, and metadata allowance | Project inode quota | Further journal file creation fails/drops; reductions continue |
| Journal message rate | 20 messages / 30 s per autonomous unit | TOML-generated journald/unit limiter | Effective namespace/unit config | Excess messages drop; Capsule health/evidence remains authoritative |
| Runtime reports | 16,777,216 B | Two 8 MiB generations | Atomic report writer | Replace oldest generation |
| Runtime report inodes | 4 | Directory, two generations, and one temporary | Project inode quota and inventory | Report waits/replaces; no spill |
| Release artifacts at deploy peak | 1,610,612,736 B | Two retained + one staging, each 512 MiB, including each release's immutable 4-KiB device-admission record | Deploy preflight/retention | Publish refuses; running release untouched |
| Release artifact inodes | 49,152 | Two retained + one staging, at most 16,384 each, including each admission-record inode | Project inode quota and deploy inventory | Publish refuses; running release untouched |
| System-mutable persistent state | 1,073,741,824 B / 65,536 inodes | All persistent writable paths outside named Bolt classes, including one fixed 8,192-B/two-inode combined active-release/runtime-mode current/staging selector pair shared by `SELECTOR-INIT-*`, `RELEASE-SWITCH-*`, `DEV-EPOCH-*`, and `ACTIVATE-*`; the selector lock reuses its parent-directory FD and launcher adoption is charged to the existing release/system inventories | Project quota, read-only root, generated `ReadWritePaths`, exact selector/launcher schema and inventory, same-filesystem/exchange preflight | Service write fails within its class; unknown path, uncharged launcher byte/inode, selector allocation, lock inode, second pair, or third selector inode cannot start |
| Selector approval/target workspace | Exactly one 4,096-B Ordinary buffer phase-reused for full target core/final record, an exact 256-B challenge, and an exact 512-B response; response is copied only to 512 B of the already-reserved 1-MiB mutator stack before buffer reuse. Zero additional target buffers, retry owners, background tasks/timers/queues, or persistent bytes/inodes before signature acceptance | TOML `max_selector_authorization_bytes=4096`; challenge layout is header 16 + expiry/reserved 16 + three hashes 96 + prerequisite root 32 + four device/inode values 32 + Ed25519 key id 32 + reserved 32 = 256 B; response adds signature 64 + reserved 192 = 512 B; unused union bytes zero | Sole selector-mutator operator invocation before any stop/mask/write; exact encoder/decoder, zero-field verifier, Ordinary-byte ledger, stack-frame assertion, and public-key verifier before lock reacquisition | Any layout/zero/size overflow, second buffer allocation, timeout, rejection, crash, stale prestate, or bad signature clears/recomputes the same slot and changes no authority; selected legacy or `CapsuleDisabled` risk management continues |
| `/tmp` and `/run` | 268,435,456 B / 16,384 inodes each | Two generated tmpfs mounts; charged within non-Bolt system memory class | Mount options and effective-unit verifier | Allocation fails within tmpfs; entry health degrades if required service cannot proceed |
| Post-migration mutable host-disk peak | 3,462,463,488 B | recovery ceiling + journal + reports + releases + system-mutable | Host inventory, project quotas, and per-device floor gate | Publish/entry blocked; risk reserve retained |
| Migration/quarantine host-disk peak | 6,146,818,048 B | post-migration peak 3,462,463,488 + sealed legacy allocated-block ceiling 2,684,354,560; no migration scratch | Cutover project quotas and sealed quarantine ownership | Autonomous start refuses above this; exceptional corrupt history remains bounded |
| Main operational / hard memory | 3,758,096,384 B / 4,026,531,840 B | Operational: 2 GiB ordinary + 512 MiB arena + 512 MiB ballast + 512 MiB overhead; hard adds unallocatable 256-MiB guard | Admission ledger and cgroup `MemoryMax` | No admission above operational ceiling; asynchronous charge crossing guard restarts safely |
| Ordinary memory ceiling | 2,147,483,648 B | One global ordinary allocator/acquisition pool; no margin class | Capped allocator, byte ledger, and cgroup admission | New risk/background work stops |
| Recovery user-space arena | 536,870,912 B | Protected queues/workspaces/buffers; allocated, touched, and retained at startup | Recovery allocator and cgroup inspection | Startup fails if charge cannot be established; ordinary allocations denied |
| Recovery physical ballast claim | 536,870,912 B | Full page-rounded maxima reserved before create/open: 128 resident stack claims = 134,217,728; 30 protected socket claims = 188,743,680; permanently touched locked reserve = 213,909,504. Invariant is `touched_free + full_active_reservations + locked = 536,870,912`, never observed current charge. Stack guard/VMA/page-table metadata and recovery/config page cache are disjoint `N_main` rows | Atomic claim substitution before thread/socket creation, generated `NetworkFootprint`, `actual<=reservation` inspection, cgroup measurement, and retouch after close/join or retained-state transfer | Equality, pre-create reservation, or component-overrun failure blocks entry/open; long-lived claims retain their full reservation |
| Main non-pool overhead `N_main` | `max(N_live,N_migration) <= 536,870,912 B` | Generated rows for native-thread guard/VMA/page-table metadata (resident 1-MiB stack claim is ballast-owned), exact-head ELF/DSO load segments, all other VMAs/page tables, allocator arenas/metadata, declared runtime objects, recovery/config cache, main-cgroup retained network rows, and the main-cgroup share of legacy directory/inode/dentry/xattr metadata. Mandatory direct I/O plus the seal makes migration source payload-data cache zero. Active socket `C` and resident stack pages are excluded because socket pools/ballast own them; only root/unmanaged retained/network/filesystem rows are in `K_host` | Generated closed-form table with one ownership tag per row from resolved TOML/build/kernel manifests, source fence, and maximum-state cgroup test | Missing coefficient/class/charge owner, duplicate ownership, subtotal overflow, payload-cache residency, buffered fallback, or observation above a row blocks; observations cannot define/enlarge the bound |
| Archive-worker operational / hard memory | 201,326,592 B / 268,435,456 B | Operational includes max object, AWS/IMDS SDK, 262,144-B cleanup response, TLS/DNS, allocator/stacks/mappings; hard adds 64-MiB guard | Separate cgroup and maximum-operation success test | Worker failure retains evidence and fixed retry; no admission credit from guard |
| Required host physical-memory claims / accepted `MemTotal` | 8,053,063,680 B claims; observed `MemTotal` in `[8,053,063,680, 8,589,934,592]` | main 4,026,531,840 + archive 268,435,456 + journal 134,217,728 + system 1,610,612,736 + user 268,435,456 + touched host reserve 1,073,741,824 + kernel/unmanaged reserve 671,088,640; upper bound fixes RAM-dependent metadata | `MemTotal`, slices, `MemoryMin`, reserve-service, unclaimed-gap, and effective-unit verifier | Host/profile startup fails outside the interval; a different RAM class requires a newly reviewed profile |
| Host reserve service | 1,073,741,824 B | Touched sacrificial pages equal two 512 MiB recovery classes | Dedicated highest-OOM-score service, auto-restart, entry health | Loss frees physical recovery capacity and blocks entry until automatically recharged |
| Kernel/unmanaged memory reserve `K_host` | 671,088,640 B | Generated closed form: pinned kernel/AMI base + CPU-count*per-CPU + device-count*per-device + bounded filesystem + bounded cgroup + global network + closed-socket retained TIME_WAIT/FIN/orphan/conntrack state that the signed kernel proves is root/unmanaged + other closed manifested classes | Signed AMI/kernel/BTF/TOML ledger, effective sysctl/socket-option verifier, and admission census | Unknown module/device/object/coefficient, charge owner, disabled cap, subtotal overflow, or reserve encroachment blocks startup/entry; runtime observation cannot set the bound |
| Main protected `MemoryMin` | 1,610,612,736 B | Recovery arena + ballast claim + non-pool overhead | Generated main unit | Drift blocks startup; ordinary 2-GiB class receives no protected credit |
| Swap | 0 B host-wide | Autonomous profile | `/proc/swaps` empty, swap/zram units absent, and effective `MemorySwapMax=0` on every generated unit/slice | Any swap device/unit or effective-unit drift blocks startup |
| Async futures hard/ordinary/recovery | 512 / 384 / 128 | `10*6 + 16*2 + 8*3 + 12 = 128` protected future demand; futures are runtime objects, not native threads | Async spawn wrapper and fixed executor queues | Ordinary future denied; protected supervisors remain; no OS-thread credit is inferred |
| Main native threads and stack/guard reservations | 128 threads / resident stacks `128*1,048,576=134,217,728 B` plus generated `N_main` guard/VMA/page-table rows | 32 runtime/blocking + 32 NT/provider + 32 SDK/DNS/TLS + 16 action workers + 16 process/native; before creation every possible thread reserves its resident stack from ballast and its disjoint guard/mapping overhead from `N_main` | Native-thread registry, exact-size thread builders, provider source fences, pre-create ballast/`N_main` claims, effective `TasksMax=128`, and effective `LimitSTACK=1,048,576` | 129th thread or any larger/unregistered stack mapping is denied; either missing claim blocks creation; async-future capacity cannot create an extra thread |
| Host tasks / file descriptions | 512 / 8,192 | Closed signed-AMI unit census; disjoint maxima main-or-migration 128/2,048, archive 16/256, journal 16/256, system 256/4,096, operator 64/1,024, reserve 4/64 | Effective `TasksMax`/`LimitNOFILE` on every unit/slice, `kernel.pid_max=512`, `fs.file-max=8192`, enabled-unit/socket/timer census | Unknown unit/socket activation, drift, or cap-plus-one blocks startup/admission; main and migration cannot coexist |
| Non-Bolt network lifetime | At most 16 generated owners / 64 simultaneous sockets; combined host retained/live projection fits fixed port, conntrack, TIME_WAIT, FIN/orphan, task, FD, memory, and `K_host` caps | Signed-AMI service census with per-owner concurrency, minimum-dial, stable-reset, backoff, and BTF-priced charge-owner rows | Host `NetworkLifetimeFootprint`, unit/socket/timer census, effective sysctls and pre-dial admission | Unknown service/retry/socket or projected host-cap overflow blocks autonomous startup/entry; fixed rows mutate in place |
| File descriptors hard/ordinary/recovery | 2,048 / 1,536 / 512 | Exact recovery decomposition below | Acquisition wrapper + systemd | Ordinary acquisition denied; protected FDs remain |
| Main WebSocket generation-owner rows / physical sockets | 16 cap / 11 populated / 16 sockets maximum | One generated generation row and at most one close/join-before-redial socket per row; autonomous per-asset unsubscribe is source-fenced unreachable | `NetworkFootprint`, connection semaphore, lifecycle supervisor, and provider source fence | Unknown/17th row, per-asset unsubscribe attempt, overlapping generation, or expired asset blocks before open |
| Connection/authentication retry owners | 16 | Closed fixed recovery groups for WebSocket generations, HTTP/RPC/relayer control, DNS/TLS, and credentials; ownership is generated explicitly rather than inferred one-per-WebSocket | Config validation and protected retry partition | Unknown/17th group is invalid; retry mutates one owner in place |
| Account capture response | 21 orders / 11 positions-or-claims, no continuation | Current maxima plus one on an exclusive account; known prepared ids query individually | Decoder and reconciliation gate | Extra item/cursor/inconsistency halts; no terminal-history scan or truncation |
| Main HTTP owner rows | 18 cap / 17 intended populated including alert, Polygon RPC, and relayer | Every HTTP client/use has one generated owner row; audited repo has 14 before all three additions | `NetworkFootprint`, client constructor, and source fence | Missing intended row, unknown, or 19th owner blocks autonomous startup/client construction |
| Main origin rows | 19 cap / 18 intended populated including alert, Polygon RPC, and relayer | Closed resolved-TOML origin ledger; audited repo has 15 before all three additions | `NetworkFootprint`, client constructors, and origin source fence | Missing intended row, unknown, or 20th origin blocks autonomous startup/pre-open |
| Main HTTP protocol / idle / redirect / proxy / library retry | HTTP/1.1 / 0 / 0 / 0 / 0 | Serial dial; Bolt retry ledger is sole owner; close/join before redial | Client builders and source fence | Drift or hidden retry/pool path blocks startup/pre-open |
| Main DNS/TLS rows / DNS sockets | 18 cap / 17 intended populated / 2 sockets | Generated hostname/session rows and two serial resolver sockets; audited repo has 14 before alert, Polygon RPC, and relayer | `NetworkFootprint`, bounded resolver/TLS stores, and socket wrapper | Missing intended row, unknown row, 19th row, or third DNS socket blocks; known rows evict/re-resolve in place |
| Main physical sockets total / protected | 34 / 30 | 16 WebSocket + 12 protected HTTP + 4 ordinary HTTP + 2 DNS; protected excludes the 4 ordinary HTTP sockets | Generated socket census, semaphores, FD wrapper, and cgroup charge inspection | Unknown/35th total or 31st protected socket blocks before open |
| Per-main-socket physical reservation `C` | 6,291,456 B | Userspace/TLS/control: `8*131,072` TLS plaintext/ciphertext + `16*65,536` protocol codec/control + `16*65,536` dial/handshake = 3,145,728; effective Linux receive `1*1,048,576`; effective send `1*1,048,576`; BTF-priced kernel objects: `16*16,384` socket/TCP + `64*4,096` sk_buff/fragment + `32*8,192` backlog/retransmit + `16*16,384` optmem/ancillary/ephemeral = 1,048,576. Thirty protected sockets reserve `C` from ballast and four ordinary sockets reserve `C` from the ordinary pool before create/open | Generated formula, pre-open claim substitution, effective Linux-doubling/rmem/wmem/autotune/backlog/retransmit/fragment/optmem/socket-option verifier, signed-kernel charge map, and post-create `actual<=reservation` inspection | Missing full reservation blocks open; component/total overrun closes the socket and blocks readiness; observations never resize or credit the reservation. On close, main-cgroup residue moves to a disjoint `N_main.net_retained` claim and root/unmanaged residue to `K_host` before `C` is released/retouched; neither releases until effective counters prove uncharge |
| Main network lifetime retained state | `D_o(H)=min(b_o+ceil(rho_o*H), 1+floor(H/delta_o))`; for each retention class `retained_o(H)<=c_o*(ceil(H/delta_o)+1)`; generated sums fit TIME_WAIT, FIN, orphan, conntrack, ephemeral-port, `N_main.net_retained`, and `K_host` caps | Per-owner concurrency `c_o`, bucket capacity/refill `b_o/rho_o`, minimum dial interval `delta_o`, stable-reset interval, pinned retention horizons, 34-live-socket cap, route/neighbour rows, two DNS-UDP sockets, TLS-session cache rows, and effective memcg/root charge ownership | `NetworkLifetimeFootprint`, fixed per-owner rows, signed-AMI/BTF/sysctl charge manifest, cgroup `memory.current`/`memory.stat sock`/`memory.events`, and pre-dial projection | Projected cap/port/main-cgroup/kernel-ledger exhaustion blocks before dial; early reset is invalid; close partitions retained ownership from `C` into `N_main.net_retained` or `K_host`; reconnect storms mutate fixed rows only |
| Main AWS credential state | IMDSv2 only: 1 generation / 1 timer / 65,536-B response / 1 in flight | Environment/shared-file/ECS/web-identity/process/default chain disabled; SDK retry zero | Explicit provider constructor, auth retry owner, and source fence | SSM/archive-dependent work waits; local reduction continues |
| WebSocket frame | 262,144 B wire / 524,288 B decoded | Per-message cap | Decoder before allocation | Reject frame; required feed degrades |
| Provider ingress protected/ordinary | 64 / 192 items; 33,554,432 / 100,663,296 B | 16 WebSocket owner rows × 4 protected frames; 524,288 B decoded cap | Partitioned byte-weighted channel | Ordinary coalesces; protected overflow reconciles |
| NT data queue | 8,192 items / 268,435,456 B | Normalized message cap | NT boundary | Coalesce; delta overflow resnapshots |
| NT risk queue protected/ordinary | 3,072 / 1,024 items; 50,331,648 / 16,777,216 B | `10*256 + 512` protected slots at 16,384 B | Partitioned NT boundary | Ordinary blocks; protected overflow latches reconciliation |
| NT execution queue protected/ordinary | 3,072 / 1,024 items; 50,331,648 / 16,777,216 B | `10*256 + 512` protected slots at 16,384 B | Partitioned NT boundary | Ordinary blocks; reduction/account overflow reconciles |
| Runtime capture queue | 1,024 items / 67,108,864 B | Non-autonomous diagnostic mode only | Capture handoff | One gap state; no unbounded sender |
| Control/provider HTTP in flight protected/ordinary | 12 / 4 | 10 risk action lanes + account reconcile + credential/capture control; four non-S3 background lanes | Partitioned Rust HTTP semaphore | Ordinary waits; each active risk retains one lane |
| Control/provider HTTP request/response | 262,144 B / 2,097,152 B each | Body caps; excludes separately bounded S3 object stream | Before body buffering | Reject and degrade dependency |
| HTTP buffers protected/ordinary | 28,311,552 / 9,437,184 B | `12/4 * (request+response)` | Recovery/ordinary byte tokens | Ordinary request waits; protected lane remains |
| S3 in flight | 1 | Single prepared object | Archive worker | Records stay local |
| Archive-worker process | 268,435,456 B / 32 async / `TasksMax=16` / 64 FDs / 2 origins / 1 live / 0 idle | Same Rust binary; S3 + numeric IMDS sequentially over HTTP/1.1; one object/credential generation; 192-MiB operational ceiling; redirects/proxies/SDK retries disabled | Separate cgroup/unit and read-only handoff | Crash/OOM retains local object; main records one fixed retry; no authority mutation |
| Alert transport | 64 latest-state bits / 1 prepared / 1 in flight / 1 connection | Closed system-health registry and configured exact HTTPS origin | Capsule delivery bits, ordinary HTTP lane, alert retry owner | Keep latest state only; retry automatically; never blocks reduction |
| Protected recovery retry owners | 64 | 20 order/action ambiguity + 10 settlement + 10 risk reconciliation + 16 explicitly generated connection/authentication groups + 8 essential system owners including replica repair | Recovery retry partition | Candidate cannot consume the owner; recovery retries in place |
| Ordinary retry owners | 32 | 14 lifecycle/book owners + 18 fixed background owners | Ordinary retry partition | Background work waits/coalesces |
| Candidate future retry claim | 4 per risk | 2 order/action + 1 settlement + 1 reconciliation; `10*4=40` within the protected partition | Atomic candidate capacity acquisition | Candidate rejected before provider preparation |
| Retry timing | 250 ms initial, 60 s maximum, factor 2, jitter ≤250 ms, request timeout ≤60 s | TOML | One scheduler | Delay/index saturate without allocation |
| Minimum autonomous market period | 300 s | S3 and episode-rate proof | Config validator | Shorter profile rejected pending new proof |
| Accepted market episodes | 288/day | 1 lane × `86,400/300` × exactly 1 accepted canonical identity/serial | Complete discovery/identity gate | Zero after trusted close may skip new risk without an episode/evidence; it never proves the market absent; multiple/wrong identities block |
| Gamma exact-slug discovery | At most 2 items and 2,097,152 B per response; `limit=2`, `offset=0`; at most 2 queries (current+next) in one 30-s recovery freshness lease | Exact discovery slug from TOML template + lane + trusted window; accepted `GammaMarketBinding` is `(Gamma id, condition id, question id, exact slug, trusted open/close window, neg-risk mode, ordered exactly-two(outcome index, normalized outcome, CLOB token id))`; the evidence id separately excludes slug/window/time | Capped HTTP decoder, binding hydrator, and lifecycle discovery gate | Exactly one fully hydrated matching binding is accepted; zero remains no episode, while two, cap overflow, missing token id, wrong binding, or later mutation blocks without resetting novelty; zero after trusted close may commit only `ClosedWindowNoAcceptedCandidate`, never absence of exposure |
| S3 terminal objects per market | 12 | 1 market + 1 system + 10 non-replenishing risk ordinals | Deterministic objectizer | Further evidence is a duplicate state or invalid registry member |
| S3 market namespace | 365 × 3,456 = 1,261,440 fixed keys | `history/ring/{000..364}/{0000..3455}` | Key builder and bucket inventory | Unknown key is invalid; conditional PUT never adds an extra key |
| S3 legacy namespace | 258 fixed keys / 2,163,350,912 B | `legacy/{000..257}`, never reused for another migration | Legacy state machine and bucket inventory | Retained indefinitely on delete outage but cannot grow |
| S3 universal retention | At most 1,261,698 objects / 3,314,119,482,752 B | 365 market slots plus one protected at-most-258-object legacy used prefix; unused fixed key positions remain absent and every prefix begins `Unverified` until its bounded empty check commits `EmptyVerified` | Never-versioned exclusive bucket, 366-prefix initial census, local-reference barrier, delete/list/empty reuse | PUT stops if version state, initial absence, reference proof, or empty barrier is unverifiable; an unexpected pre-existing key integrity-halts before upload |
| S3 cleanup cursor/response | Numeric `0..3456` market or `0..258` legacy; delete batch ≤64; response ≤262,144 B; final list ≤1 key | Finite generated key namespace; no opaque continuation token | Capsule cursor, capped decoder, HEAD-per-key and final `MaxKeys=1` | Crash repeats one idempotent batch/HEAD; unexpected key integrity-halts without growth |
| Restart cadence | 30 s constant, infinite attempts | TOML `Restart=always`, `RestartSec=30s`, `StartLimitIntervalSec=0` | Generated/effective systemd unit | Retry forever without attempt state/operator reset; journal rate cap contains logs |

Autonomous mode disables NT state load/save, streaming catalog persistence, raw
runtime capture, file logging, and core dumps. Those cannot create additional
mutable paths beside this table. Non-autonomous diagnostic profiles may enable the
bounded capture queue, so its memory is included in the worst-case ordinary budget.

## Capsule Section Accounting

The encoder treats section maxima as limits from resolved TOML and pads the payload
to `P`. Reserved padding is not an overflow area and cannot be allocated without a
new reviewed schema and migration.

| Capsule section | Exact maximum |
|---|---:|
| Header and global control | 65,536 B |
| Ten risk slots × 16,384 | 163,840 B |
| Twenty order slots × 12,288 | 245,760 B |
| Ten settlement slots × 16,384 | 163,840 B |
| Thirteen episode slots × 8,192 | 106,496 B |
| Fourteen lifecycle/book slots × 4,096 | 57,344 B |
| Receipt/ready bitsets, one 32 KiB prepared record, one prepared-object descriptor, retry/health/migration state | 65,536 B |
| 365 fixed 16-byte S3 ring descriptors plus cursor/legacy state/padding | 8,192 B |
| Account-global Safe nonce lane, including two 4,096-B exact body buffers | 16,384 B |
| Encoded maximum | 892,928 B |
| Zeroed schema reserve | 155,648 B |
| Total `P` | 1,048,576 B |

CI constructs the maximum legal value of every bounded identifier, collection,
request, and evidence variant and asserts both its section maximum and the complete
payload size. There is no fallback serialization.

Each 12,288-byte order slot includes exactly 2,048 bytes for a sorted unique set of
at most 64 canonical 32-byte fill transaction hashes. The count, phase, request,
provider certificate metadata, quantities, finality cursor, and compact aggregate
proof fit the remaining 10,240 bytes. Receipts/log bodies are never stored in the
Capsule and are fetched sequentially into the already bounded protected HTTP
buffer. The maximum-order constructor fills all 20 hash sets simultaneously.

Each 16,384-byte risk slot dedicates 8,192 bytes to 64 fixed 128-byte pending
semantic receipts and 8,192 bytes to bounded workflow state. The market episode
uses 4,096 bytes for 256 fixed 16-byte receipts; the system episode uses 4,096
bytes for 64 fixed 64-byte receipts. The remaining episode bytes hold novelty and
lifecycle metadata. Tests construct every receipt simultaneously; this accounting
does not assume the arena is writable.

## Memory Accounting

Every ordinary heap allocation and typed acquisition consumes the one global
ordinary pool before allocation. Queue/cache/body sublimits are stricter ownership
partitions inside that pool; their sum and the fixed runtime remainder equal exactly
2 GiB. Item caps alone do not prove this table.

| Ordinary class | Budget |
|---|---:|
| Base Rust/NT runtime and fixed latest-value caches | 805,306,368 B |
| NT data queue | 268,435,456 B |
| Ordinary NT risk/execution partitions | 33,554,432 B |
| Bounded diagnostic capture | 67,108,864 B |
| Ordinary provider ingress | 100,663,296 B |
| Ordinary HTTP buffers | 9,437,184 B |
| Ordinary task futures and declared owned buffers | 201,326,592 B |
| Fourteen book/cache arenas | 134,217,728 B |
| Serialization and allocator scratch | 268,435,456 B |
| Four ordinary HTTP socket claims (`4 * 6,291,456`) | 25,165,824 B |
| Global allocator fixed-runtime remainder | 233,832,448 B |
| Ordinary ceiling | 2,147,483,648 B |
| Recovery user-space arena | 536,870,912 B |
| Recovery physical ballast | 536,870,912 B |
| Main non-pool overhead allowance | 536,870,912 B |
| Operational ceiling | 3,758,096,384 B |
| Non-allocatable cgroup guard | 268,435,456 B |
| `MemoryMax` hard total | 4,026,531,840 B |

Migration and live runtime are mutually exclusive. While the live runtime is
stopped, the migrator takes one fixed 134,217,728-byte slice of the same ordinary
pool: 33,554,432 bytes for aligned direct-I/O input, 41,943,040 bytes for at most
1,048,576 fixed 40-byte semantic descriptors, 1,048,576 bytes for 16,384 fixed
64-byte path descriptors, 8,392,704 bytes for one archive object, 33,554,432 bytes
for Feather/decoder validation, 10,485,760 bytes for `16,384*640` source
reopen/egress metadata,
and 5,238,784 bytes for join/key/slack. Every source read/reread uses aligned
`O_DIRECT`/`RWF_DIRECT`; preflight verifies `STATX_DIOALIGN`, aligned tail handling,
and no buffered fallback. The single-link/no-mapping seal, fadvise, and mincore
proof makes source payload-data cache zero; directory/inode/dentry/xattr state is
separately capped at 134,217,728 bytes and charged by effective owner. This is
reuse inside the 2-GiB ceiling, not an additive host claim; migration cannot allocate
a second workspace, spill, or create a dirty local file.

The 536,870,912-byte recovery arena has this exact suballocation:

| Protected arena class | Budget |
|---|---:|
| Ten risk closure workspaces (`10 * 16 MiB`) | 167,772,160 B |
| Sixteen WebSocket-generation owner buffers (`16 * 8 MiB`) | 134,217,728 B |
| Protected NT risk and execution partitions | 100,663,296 B |
| Protected provider ingress | 33,554,432 B |
| Twelve protected HTTP buffers | 28,311,552 B |
| Capsule/receipt/venue-snapshot buffers | 33,554,432 B |
| Protected async futures and typed scratch | 33,554,432 B |
| Recovery allocator metadata/slack | 5,242,880 B |
| Arena total | 536,870,912 B |

The separate 536,870,912-byte ballast is an anonymous page-aligned mapping that is
allocated, touched, and retained before provider connection/recovery work. It is
not an allocator. Protected non-arena claims are:

```text
128 native resident-stack claims * 1,048,576 = 134,217,728
30 protected socket claims * 6,291,456        = 188,743,680
non-releasable reviewed locked reserve         = 213,909,504
total                                           = 536,870,912
```

At every instant, touched free-ballast pages plus full page-rounded active
reservations plus the locked reserve equal exactly 536,870,912 bytes. Each
protected acquisition atomically substitutes its complete reviewed charge for
touched pages before the thread/socket is created and retouches those pages only
after close/join and charge-owner retention transfer. A later measurement
may prove only `actual <= reservation`; it never reduces the active claim. The
128-thread hard registry and `TasksMax=128` ensure every possible native stack has
one full 1,048,576-byte resident claim. Guard pages, stack VMAs/page tables, and all
recovery/config file page cache are charged separately and exactly once in
`N_main`.

Each socket claim `C=6,291,456` is generated without an opaque slab:

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

The 30 protected owners are 16 WebSocket generation owners, 12 protected HTTP,
and two DNS sockets. Four ordinary sockets reserve the same `C` from the ordinary
pool. Linux buffer doubling, receive/send maxima, autotuning, backlog,
retransmission, fragment, and optmem caps are pinned by the signed-AMI/TOML
profile and verified before the first open. On close, socket-derived TIME_WAIT,
FIN, orphan, conntrack, and ephemeral-port residue is partitioned by the effective
signed-kernel charge owner. Main-cgroup bytes retain a generated
`N_main.net_retained` claim until `memory.stat sock` and object counters prove
uncharge; only root/unmanaged bytes enter `K_host`. Route/neighbour, DNS/UDP, and
TLS-session rows follow the same charge-owner rule.
Unknown claim kinds cannot replace ballast, and no page-cache eviction earns
admission credit.

Before any provider connection or recovery read, startup verifies the complete
arena and ballast equality/charges. Swap is zero. The 512-MiB non-pool claim is
separate from the exact 2-GiB ordinary allocator ceiling and is the generated
closed form:

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

Every generated row records its unit, count, coefficient, build/kernel/config
manifest digest, ownership tag, and subtotal. Resident stack pages are owned only
by ballast; active socket userspace/kernel bytes only by `C`; closed network
residue only by `K_host`; ordinary/recovery allocator payloads only by their pools.
The generator rejects a missing tag or any pair of overlapping owners and proves
the actual resolved sum is at most the fixed cap. Runtime observations are drift
checks only, never the source of the limit. Admission also reserves every possible remaining typed/ordinary growth under
the 3.5-GiB operational ceiling. Any asynchronous bypass first consumes the
unallocatable 256-MiB guard and then hits the 3.75-GiB cgroup boundary rather than
consuming a protected claim. Entry remains blocked if any inactive claim is not
retouched, any active claim exceeds its reservation, or maximum-state non-pool
charge exceeds 512 MiB.

The S3 SDK is outside the main cgroup in the same Rust binary's archive-worker mode.
Its entire object/SDK/IMDS/TLS/DNS/allocator/stack/mapping state is mechanically
bounded by a 201,326,592-byte operational ceiling under
`MemoryMax=268,435,456`, a separate 32-future async semaphore, native-OS-task
`TasksMax=16`, `LimitNOFILE=64`,
two exact origins used through one sequential live connection, no idle connection,
and one object. It has read-only prepared bytes and cannot commit authority. The
gate is a successful maximum 8,392,704-byte upload plus maximum capped
delete/HEAD/list and IMDS operation inside the operational ceiling, not merely
survival after an OOM.

The whole-host sum is exact:

| Host memory class | Hard bound |
|---|---:|
| Bolt main hard cgroup claim | 4,026,531,840 B |
| Rust archive worker hard cgroup claim | 268,435,456 B |
| Journal process | 134,217,728 B |
| Non-Bolt system services | 1,610,612,736 B |
| User/operator slice | 268,435,456 B |
| Touched sacrificial host reserve | 1,073,741,824 B |
| Kernel/unmanaged unclaimed reserve | 671,088,640 B |
| Required claim subtotal / minimum accepted `MemTotal` | 8,053,063,680 B |

Bolt receives `MemoryMin=1,610,612,736` and protected OOM preference. The host
reserve has the highest OOM score, retries automatically, and gates entry until its
pages are recharged. Exactly 671,088,640 bytes remain intentionally unclaimed for
kernel and unmanaged charges. Its generated closed form is:

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

The signed AMI/kernel/BTF manifest and resolved TOML provide every count and
coefficient. `memtotal_max_bytes=8,589,934,592`; a larger observed host is rejected,
and only cache not already charged to a named cgroup enters `K_host`. The ownership
generator proves `K_host` excludes active `C` socket
claims and every `N_main`/cgroup-owned byte, and proves the actual resolved subtotal
is at most the fixed cap. Encroachment gates entry. Startup/effective-unit inspection
rejects a host outside `[8,053,063,680,8,589,934,592]`; the audited stopped 4 GiB
host is incompatible. Provider input limits, queue caps, lifecycle eviction, task
joining, the generated main `NetworkFootprint`, the two-origin archive registry,
HTTP/1.1 serial dial, zero idle/redirect/proxy/library retry, and fixed
DNS/TLS/credential rows prevent repeated events from increasing the claimed set.

## Protected Closure-Capacity Accounting

Every candidate acquires this vector with its fixed request and candidate snapshot
in the `EntryPreparedNotAuthorized` transition:

| Per-risk protected claim | One risk | Ten risks |
|---|---:|---:|
| Risk/order/settlement/episode slots | `1 / 2 / 1 / 1` | `10 / 20 / 10 / 10` |
| Evidence offsets | 64 | 640 |
| Recovery workspace | 16,777,216 B | 167,772,160 B |
| Async task owners | 6 | 60 |
| FD owners | 8 | 80 |
| Protected HTTP lane | 1 | 10 |
| Protected NT risk queue slice | 256 items / 4,194,304 B | 2,560 / 41,943,040 B |
| Protected NT execution slice | 256 items / 4,194,304 B | 2,560 / 41,943,040 B |
| Future retry owners | 4 | 40 |

The remaining protected capacity is assigned before startup/admission, not offered
to ordinary work:

- async futures: `10*6 + 16 WebSocket generation owners*2 + 8 essential owners*3 + 12 fixed executors =
  128`; the fixed executors are the quorum writer, receipt materializer, two
  venue-capture tasks, three voter probes, clock probe, retry scheduler, health
  adjudicator, quota probe, and archive handoff;
- native threads: 32 runtime/blocking + 32 NT/provider + 32 SDK/DNS/TLS + 16
  action workers + 16 process/native = 128 exactly; the native-thread registry and
  main-unit `TasksMax=128` forbid a 129th, while async futures stay on these
  executors and do not count as native-stack capacity;
- FDs: 80 risk lanes + 136 generated owner-row reservations
  (`(16 WebSocket generation + 18 HTTP)*4`) + 64 account/settlement HTTP + 32
  AWS/SSM/IMDS + 64 Capsule/arena/witness/quota + 48 lifecycle/discovery + 88
  runtime/native = 512;
- WebSocket connections: 16 owner rows × one close/join-before-redial physical
  socket = 16;
- protected HTTP: ten risk lanes + one account-wide reconcile + one
  credential/current-capture control = 12;
- protected socket ballast: 16 WebSocket + 12 protected HTTP + two DNS sockets are
  `30*6,291,456=188,743,680` bytes; the four ordinary HTTP sockets have an explicit
  `25,165,824`-byte ordinary claim, making 34 physical main sockets total. Every
  socket reserves the complete expanded `C` formula before create/open; Linux
  doubling/autotune/queue limits are effective-config gates, and post-create
  inspection may prove only `actual<=C`;
- protected queue remainder: 512 risk and 512 execution items for account/system
  reconciliation, so each protected queue is `2,560+512=3,072`; provider ingress is
  `16 WebSocket generation owners*4=64` frames;
- recovery retries: 20 order/action ambiguity + ten settlement + ten risk
  reconciliation + 16 explicitly generated connection/authentication groups +
  eight essential system = 64. The
  eight system owners are venue-account truth, AWS/SSM authentication, replica A,
  replica B, witness/quorum repair, trusted clock, S3 upload, and S3 retention.

The generated `NetworkFootprint` is the common source for these socket, FD, owner,
origin, DNS/TLS, client, and task rows. Every connect, client construction,
network-helper spawn, and raw-socket call consumes one declared row before open;
unknown ownership fails closed. Main HTTP uses 1.1, serial dial, zero idle,
redirect, proxy, or library retry, and close/join before redial.

The companion `NetworkLifetimeFootprint` owns the state after close. For every
owner `o`, resolved TOML supplies concurrency `c_o`, bucket capacity/refill
`b_o/rho_o`, minimum dial interval `delta_o`, and a stable interval before its one
bucket may reset. For every signed-AMI retention horizon `H`:

```text
D_o(H) = min(b_o + ceil(rho_o*H), 1 + floor(H/delta_o))
retained_o(H) <= c_o * (ceil(H/delta_o) + 1)
```

Generated sums separately cover TIME_WAIT, FIN, orphan, conntrack, ephemeral-port
occupancy, route/neighbour entries, two DNS-UDP sockets, TLS-session cache, and
BTF-priced memory. The ephemeral sum must fit the configured port interval after
its fixed reserve. Before `C` is released or ballast retouched, the signed charge
map transfers main-cgroup residue to the corresponding
`N_main.net_retained` row and only root/unmanaged residue to `K_host`; a pre-dial
projection that would exceed either ledger, any row, port, or kernel subtotal
blocks the dial. Neither retained claim releases before its effective counters
prove uncharge. Stable reset mutates one fixed row and never discards retained
ownership early.

The ordinary retry enum has exactly 32 owners: 14 lifecycle/book bundles plus these
18 named background owners: market discovery, next-market preparation,
closed-window skip, alert delivery, journal probe, report publication, release
inventory, capture flush, capture cleanup, legacy-raw archive, legacy-recovery
archive, cache sweep, config-drift probe, quota probe, provider-metadata refresh,
metrics flush, health snapshot, and operator-status export. Unknown owners cannot
allocate a generic slot.

The generated accounting test sums this table from resolved TOML and asserts every
ten-risk total is less than or equal to its protected partition before any entry
command reaches the provider. Saturation tests consume every ordinary and fixed
remainder token, then drive all ten risks through exit, reconciliation, settlement,
evidence receipt, and terminal states.

## S3 Retained-History Accounting

The worst-case proof uses configured maxima rather than observed trading frequency.
Risk ordinals are a non-replenishing per-market infrastructure budget, not a
strategy signal or concurrent-position count. Operational health transitions reuse
the same market's fixed system-state registry and never open alert episodes. The
complete discovery gate accepts exactly one identity per lane/serial; zero creates
no episode and identity churn cannot increase the 288/day term. Serial wrap is an
adjacent modular transition during continuous operation; constant-work downtime
rebase creates no missed-window episode and therefore adds no rate term.

```text
markets/day       = 86,400 / 300 = 288
records/market    = 256 market + (10 risks * 64) + 64 system = 960
records/day       = 288 * 960 = 276,480
objects/market    = 1 market + 1 system + 10 risk = 12
objects/day       = 288 * 12 = 3,456
market-ring objects = 365 * 3,456 = 1,261,440
legacy-slot objects <= ceil(2,162,294,144 / 8,388,608) = 258
global object maximum = 1,261,440 + 258 = 1,261,698
```

With every record at its padded 32,768-byte maximum and every object carrying a
4,096-byte archive envelope:

```text
cohort_S3_bytes_max = 365 * ((288 * 960 * 32,768) +
                            (288 * 12 * 4,096))
                    = 3,311,956,131,840 bytes

one_time_legacy_archive_ceiling = 2,162,294,144 classified/exact-byte payload
                                + 258 * 4,096 envelopes
                                = 2,163,350,912 bytes

global_S3_bytes_max = 3,311,956,131,840 + 2,163,350,912
                    = 3,314,119,482,752 bytes
```

This is deliberately conservative and potentially expensive; it is still a real
bound. The publish verifier reports both this maximum and the actual encoded usage
forecast so an operator can change retention or semantic cardinality through a new
reviewed proof. Compression cannot be credited in the safety bound.

The legacy term is part of the global bound for however long upload/revalidation
takes; no completion-time assumption removes it. Its 365-day clock starts only at
quorum-durable `DeletionAuthorized`, after bounded all-object remote revalidation.
Remote deletion additionally requires durable `LocalEgressDeleted`, so S3 cannot
expire while any egress source still needs it as deletion evidence. Migration has
no derived local output or progress authority. After `DeletionAuthorized`, only
egress paths are removed under the continuing fence; `LocalEgressDeleted` requires
every egress path absent/parents synced and every permanently bounded quarantine
path still present with its frozen digest/allocation.

Pruning is a prerequisite for PUT. The bucket is dedicated, has never had
versioning enabled, and `GetBucketVersioning` must return no status; `Enabled` and
`Suspended` are invalid. The prefix has exclusive IAM write/delete authority. The
365 market cohorts are fixed ring slots with 3,456 final key positions; a
conditional request targets one of those positions and therefore adds no extra key.
Every never-owned prefix first passes `Unverified -> EmptyVerified` through one of
exactly 366 `ListObjectsV2(MaxKeys=1)` checks. Reusing one requires
`Owned -> ReuseBlocked -> DeletePrepared -> Deleting -> VerifyingEmpty ->
EmptyVerified -> Owned`; a trusted UTC day may rebase the fixed ring cursor, but
neither a wrapping day tag nor downtime permits reuse without zero local references
and the empty barrier. The legacy slot uses the delete/list barrier only after its
protected period and then becomes permanently `Retired`. Lifecycle is defense in
depth and is not part of the proof. A late unresolved risk pins its original cohort
and blocks new archive-day admission until its final object is acknowledged and the
slot can be emptied. An S3 deletion outage stops further remote growth, then the
fixed local arena stops new risk. Existing-risk evidence remains within its local
reserve.

## Admission Predicate

`entry_ready` is true only when all of these are true in one evaluated snapshot:

```text
capsule_quorum_valid_and_all_three_voters_identical_for_entry
both_arena_replicas_identical
venue_reconciled
no_unknown_entry_or_authority_divergence
exact_order_authority_status_truth_table_and_polygon_finality_horizon_valid
account_capture_exact_decimal_scaled_integer_and_all_positive_dust_complete
candidate_capacity_and_market_ordinal_available_for_prepare_or_held_by_same_snapshot
unknown_integrity_evidence_fence_absent_or_candidate_strictly_beyond_it
every_device_byte_and_inode_future_claim_predicate_meets_floor
ordinary_memory/task/fd/queue/cache/connection claims fit
memory_current_plus_all_remaining_claim_growth_fits_operational_ceiling
ballast_touched_plus_full_precreate_reservations_plus_locked_equals_512_MiB
generated_N_main_actual_sum_at_most_512_MiB_with_exclusive_ownership
generated_K_host_actual_sum_at_most_640_MiB_with_exclusive_ownership
host_reserve_charged_and_MemTotal_inclusive_between_8_053_063_680_and_8_589_934_592
all_project_quotas_mounts_and_inode_limits_effective
generated_NetworkFootprint_owner_origin_DNS_TLS_socket_counts_valid
generated_NetworkLifetimeFootprint_retained_state_and_ephemeral_projection_valid
HTTP_1_1_zero_idle_redirect_proxy_library_retry_and_serial_redial_valid
IMDS_credential_and_retry_config_valid
trusted_time_brackets_same_current_unexpired_market
GammaMarketBinding_complete_exact_and_unambiguous
EvidenceEpisodeId_excludes_slug_window_timestamp_and_matches_stable_venue_semantics
all_required_polymarket_assets_observed_in_current_generation
fresh_book_for_every_required_target
all_other_required_feeds_fresh
pending_archive_integrity_valid
retention_local_reference_and_remote_empty_barriers_verified
archive_bucket_versioning_status_absent
SSM/authentication valid
provider_manifest_current_collateral_adapter_CTF_neg_risk_relayer_Safe_ABIs_valid
provider_permanent_order_hash_tombstone_capability_valid
provider_terminal_certificate_max_64_hashes_and_receipt_caps_valid
provider_relayer_explicit_competing_same_nonce_conformance_valid
transition_registry_generated_digest_bijection_and_effect_census_valid
```

The candidate snapshot, fixed signed request/hash, and full reservation are
committed together as `EntryPreparedNotAuthorized`. It has no send authority.
Immediately before a first send, Bolt re-evaluates trusted time, market/expiry,
Gamma identity, current-generation Polymarket observation/fresh books, every other
feed/health predicate, and the still-held capacity against that same candidate
snapshot, then commits the finalized pre-dispatch block, final predicate digest,
exact request/hash, and `DispatchMayHaveStarted` in one all-three transition before
the syscall. If the unsent candidate is stale or expired, all three instead commit
`EntryAborted`; no capacity releases until that abort is durably terminal.
Repairing W after A+B prepared the child only restores
`EntryPreparedNotAuthorized` and never authorizes or sends it. A crash after the
final commit is deliberately ambiguous even if the syscall had not begun.

After a crash or unknown provider result, Bolt retains the full reservation and
accepts only the exact `ProviderTerminalCertificate`. `Filled` must provide its
complete sorted unique at-most-64 set of canonical 32-byte transaction hashes for
sequential at-most-2,097,152-byte/4,096-log receipt/log/post-state proof;
`PermanentlyTombstonedNoEffect` must provide the permanent linearizable hash fence
plus untouched finalized V2 status and exact post-state. There is no
risk-increasing replay after `DispatchMayHaveStarted`. A 404/absence,
cancel/not-canceled response, elapsed FOK horizon, unsigned wire expiration,
ordinary order/trade status, or quiet chain remains Unknown. The current V2
provider lacks the required tombstone contract, so generated activation must keep
autonomous entry disabled rather than substituting a timeout or operator repair.

Risk reduction and settlement do not require `entry_ready`. They require a current
full-replica two-vote quorum plus only the specific external dependency needed to
make progress. While either is unavailable they
remain scheduled in their fixed reserved states, retry with capped backoff, and
resume without an operator.
