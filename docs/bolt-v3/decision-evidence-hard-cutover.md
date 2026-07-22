# Decision-evidence hard cutover

This cutover retires the pre-cutover decision-evidence stream. The current binary does not decode, migrate, truncate, archive, or delete that stream. Pre-cutover automated recovery and offline projection continuity are intentionally unavailable after activation.

## Activation status

Live activation is blocked until the venue/account boundary can independently prove all of the following after trading is disabled:

- zero open orders;
- zero pending acknowledgements, cancels, or fills through an authoritative event barrier;
- zero open positions and outcome-token balances;
- zero pending redemption, settlement, booking-error, or terminal-transition work;
- reconciled collateral, wallet, venue, and account state.

The current provider snapshot does not prove the complete fill and settlement barrier. An empty evidence file, an empty NT cache, or a successful old-reader pass is not a substitute.

## Operator sequence

When the independent proof above exists:

1. Disable trading, stop and mask the service, and confirm no Bolt process remains.
2. Capture two identical authoritative venue/account snapshots separated by the confirmed event barrier. Refuse the cutover on any disagreement.
3. Verify the durable kill-switch state separately; decision evidence is not halt authority.
4. Move the configured retired evidence file, the retired binary, and the retired config to an inert archive outside the active data root. Use a same-filesystem atomic rename, sync the archive and parent directory, record SHA-256 checksums and ownership, and make the archive read-only. Never delete it as part of this cutover.
5. Install the immutable current binary and the current config. Remove retired binary/config selection from the service host.
6. Start normally. Startup must refuse activation if any configured retired path exists or if the current machine stream contains an old, unknown, malformed, blank, torn, or oversized record.
7. After the first current machine record is written, rollback is prohibited. Pause and forward-fix only.

The runtime must never perform step 4. A normal start or restart only validates the permanent current-stream invariant and obtains fresh in-process readiness.

## Active streams

The current config owns two distinct paths:

- `persistence.decision_evidence.machine_relative_path`: recovery-bearing facts; scanned fail-closed at every startup and bounded by `recovery_evidence_max_bytes`.
- `persistence.decision_evidence.observation_relative_path`: diagnostic/state observations; never opened by startup recovery.

Every identity is current-only and resolved by exact `(kind, schema_version)` equality. Foreign content is corruption, not compatibility input.

## Accepted loss and retained risks

Accepted at the cutover boundary:

- no pre-cutover automated recovery;
- no pre-cutover Shadow PnL or forensic projection continuity;
- no runtime decoder for archived bytes.

Still unresolved:

- machine-stream capacity remains a whole-file limit until #1385 adds bounded crash-safe retention;
- observation disk growth remains until rebuilt novelty suppression lands;
- archive retention duration remains an owner/compliance decision, so archival is mandatory and deletion is outside this procedure.
