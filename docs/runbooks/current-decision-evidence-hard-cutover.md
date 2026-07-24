# Current Decision-Evidence Hard Cutover

## Status

This runbook describes the required cutover boundary; it does not authorize a live cutover. The repository cannot yet independently prove that every accepted order event through a pause cutoff has no pending acknowledgement, fill, cancel, redemption, booking, or settlement transition. Keep the service stopped unless that external proof and the normal deployment gates are both complete.

Pre-cutover decision evidence is an inert archive after cutover. The current runtime never migrates, repairs, deletes, or decodes it.

## Preconditions

Before touching evidence files, the operator must:

1. Stop and mask the service, verify that no Bolt process remains, and prevent automatic restart.
2. Stage and validate the immutable new binary and its current config without starting trading.
3. Obtain two matching authoritative venue/account snapshots separated by a venue-confirmed event barrier and a quiet window.
4. Prove from sources independent of the evidence file that there are:
   - no open orders;
   - no pending acknowledgements, cancels, fills, or submit reservations;
   - no open outcome-token positions;
   - no unresolved, redeemable, or pending-settlement positions;
   - no unresolved booking errors or terminal transitions;
   - no reserved or locked capital;
   - no unreconciled wallet, collateral, venue, or account state.
5. Confirm that the refreshed NT cache agrees with the authoritative venue/account snapshot. Any disagreement blocks cutover.
6. Inspect the durable kill-switch state independently. An active halt whose investigation depends on pre-cutover evidence blocks cutover.

Do not use the evidence file itself to prove quiescence. It may be inspected for forensics, but it is not readiness authority.

## Archive Boundary

The operator, never the trading binary, performs archival:

1. Resolve the configured retired path beneath `persistence.catalog_directory`; reject symlinks, non-regular files, ownership mismatch, destination collision, or any path outside that root.
2. Record the old binary and config identities and compute a SHA-256 digest for each pre-cutover evidence file.
3. Synchronize the evidence file and its containing directory.
4. Atomically rename the file on the same filesystem into a timestamped archive outside the configured data root.
5. Synchronize both source and archive directories, write the checksum/retention manifest, and make the archive read-only.
6. Remove the old binary and old config from active deployment locations. The service unit must select only the immutable new binary and current config.

The archive is never restored into an active path. Offline access uses the archived binary and config at the archived revision, isolated from the trading runtime.

## Current Generation

Leave both configured current stream paths absent. `DecisionEvidenceRuntime::open` parses the complete
path topology, opens and exclusively locks the catalog descriptor, creates every missing parent
component and both stream files through that same descriptor-relative authority, synchronizes each
namespace before use, applies private permissions, and validates the retained descriptors before
exposing component-scoped append capability. A second process using the catalog fails before namespace
mutation. Operators must not pre-create a parallel layout through pathname-based tooling.

Startup must refuse:

- any configured retired path that still exists;
- old, unknown, mixed, malformed, blank, torn, or observation identities in the machine stream;
- a machine stream above `recovery_evidence_max_bytes`;
- symlinked, non-regular, noncanonical, aliased, ancestor-conflicting, or out-of-root stream paths;
- duplicate or contradictory settlement terminal outcomes, invalid reservation/fill relationships, or
  an NT open order without attribution in an atomic admitted fact;
- any mismatch between the accepted raw venue open-order set and the unique reconciled NT open-order
  set, or failure to rebuild capital admission from that attested cache.

The observation stream is not recovery input and never supplies readiness authority. Startup validates any retained observation content through the same exact-current framing, identity, sink, gate, and payload decoder. Invalid observation content is preserved byte-for-byte, reported as a poisoned observation sink, and does not block machine recovery or activation. The runtime never repairs, truncates, replaces, or extends that poisoned stream.

## First Boot and Point of No Return

1. Start the new service through the normal launch path; do not add a cutover mode or persisted readiness marker.
2. Verify that startup validated an empty current machine stream and that both current streams have private permissions.
3. Before arming, verify the same authoritative venue/account flatness and all ordinary readiness gates again.
4. After the first trade cycle, decode the current machine stream with the current binary and confirm recovery facts are readable.

Archival begins the point of no return. Once archival starts, rollback is prohibited: remain paused, preserve both generations, and forward-fix. Never restore the retired binary, config, or evidence authority.

Any machine-stream write or sync error is commit-indeterminate: some or all bytes may exist. The runtime permanently poisons that machine sink for the process lifetime and refuses later appends without touching the file. Stop the service, preserve the active stream, and diagnose it offline. A torn or otherwise invalid machine stream remains a fail-closed startup condition; recovery is pause, archive under the governed ceremony, and forward-fix—never retry, truncate, skip, or restore retired authority.

A poisoned observation stream never gates machine recovery, but it is a terminal state for that active
observation pathname. Startup corruption and the first mid-run write or sync failure publish an
immediate typed health transition after the sink lock is released and appear on
`BoltV3OperatorHealthSurface.decision_evidence_observation`. Machine-sink poison likewise appears on
the machine field and makes blocking evidence operations fail closed. The recorder remains the sole
mutable poison authority. The runtime preserves the bytes and refuses to extend the poisoned stream.
Stop the service, preserve and checksum it outside the active data root, then forward-fix to a fresh
configured pathname under an issue-bound operator change. Do not truncate, repair, replace in place,
or silently resume the poisoned sink.

On controlled shutdown, action ingress and every evidence-producing subscription/task stop and join
before the recorder closes. Closing rejects new operations, waits for accepted append and health
publication operations. The subsequent recorder drop closes both streams and releases the catalog
lock. Do not treat a process
whose catalog lock is still held as stopped.

## Accepted Losses and Remaining Gates

The current runtime cannot recover or query pre-cutover decision evidence. Entry-chain and Shadow-PnL continuity across the boundary is intentionally lost. These accepted losses do not authorize losing active trading, settlement, accounting, or legally required records.

This cutover resets the machine-stream capacity clock but does not implement rotation, retirement, durable ordinals, or restart exact-once. Issue #1385 remains the owner of that work. Operations must monitor the active machine-stream byte size against `recovery_evidence_max_bytes`: crossing the cap is detected at the next startup and deliberately fails closed, so approaching the cap requires a planned pause and the governed archive ceremony before restart. Observation and machine stream separation prevents observation volume from consuming recovery capacity; observation disk growth still requires monitoring and its separately owned novelty/retention work.
