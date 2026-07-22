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

Create the parent directory for the configured current machine and observation streams. Leave both stream paths absent; `DecisionEvidenceRuntime::open` creates them with private permissions and validates the retained descriptors before exposing append capability.

Startup must refuse:

- any configured retired path that still exists;
- old, unknown, mixed, malformed, blank, torn, or observation identities in the machine stream;
- a machine stream above `recovery_evidence_max_bytes`;
- symlinked, non-regular, aliased, or out-of-root stream paths.

The observation stream is not recovery input and never supplies readiness authority.

## First Boot and Point of No Return

1. Start the new service through the normal launch path; do not add a cutover mode or persisted readiness marker.
2. Verify that startup validated an empty current machine stream and that both current streams have private permissions.
3. Before arming, verify the same authoritative venue/account flatness and all ordinary readiness gates again.
4. After the first trade cycle, decode the current machine stream with the current binary and confirm recovery facts are readable.

The first current machine record is the point of no return. Before it, the operator may remain paused and restore the complete archived deployment set. After it, rollback is prohibited: stop, preserve both generations, and forward-fix.

## Accepted Losses and Remaining Gates

The current runtime cannot recover or query pre-cutover decision evidence. Entry-chain and Shadow-PnL continuity across the boundary is intentionally lost. These accepted losses do not authorize losing active trading, settlement, accounting, or legally required records.

This cutover resets the machine-stream capacity clock but does not implement rotation, retirement, durable ordinals, or restart exact-once. Issue #1385 remains the owner of that work. Observation and machine stream separation prevents observation volume from consuming recovery capacity; observation disk growth still requires monitoring and its separately owned novelty/retention work.
