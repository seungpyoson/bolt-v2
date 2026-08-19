# Contract: `pump-research` Command Interface

The implementation exposes one Rust CLI, `pump-research`, with typed subcommands.
Commands receive the authoritative experiment TOML path plus artifact references
or non-semantic output locations. Semantic experiment values are not accepted as
flags or environment overrides.

The effective actor and credential scope come from the authenticated execution
principal and the TOML role binding. A command argument or payload cannot assign
its own role. Non-test execution rejects fixture principals, fixture timestamp
verifiers, and test artifact roots.

Non-test commands obtain caller identity from AWS STS `GetCallerIdentity` and
match the returned account/principal/role shape to the committed TOML binding.
Failure to obtain or match identity is a typed fail-closed outcome.

## Subcommands

### `validate`

Parse and validate the complete definition, resolve referenced registry entries,
produce canonical semantic bytes in memory, and report only identifiers, hashes,
counts, statuses, and validation errors. It performs no provider or NT run.

### `register-version`

Require an authorized append role and expected parent pointer, then atomically
register the immutable definition and Artifact Index event. Reject a stale parent,
unknown role, dirty artifact, or semantic duplicate with conflicting lineage.

### `register-source`

Validate a staged source evidence packet and retained artifact. Record acceptance
or rejection; never download provider data. Canonical acceptance requires active
rights, fidelity, coverage, retention, cost-status, and expiry evidence.
Before verified G, the command rejects any byte or metadata access that could
enter E0; E0 ingestion and coverage computation run only through the committed
custodian operation after G.

### `commit`

Create one typed G, D, C, E, or P commitment. Verify its prerequisites, current
custody checkpoint, independent timestamp receipt, role authority, and expected
experiment state. Missing or invalid evidence fails closed.

### `append-custody-event`

Append one typed access/disclosure/execution/credential event against an expected
head. It never prints credential material or result-bearing payloads.

### `close-checkpoint`

Fence the relevant role scope, bind a verified timestamp receipt to the observed
head, conditionally confirm the head is unchanged, and consume a single-use
authorization. A changed head returns stale status without advancing state.

### `discover`

Require verified G and D, active admitted discovery inputs, and complete partition
eligibility. Produce the deterministic roster, episode, control, censoring, and
attrition manifest. It accepts no threshold or window override.

### `confirm`

Require verified G/D/C and a closed pre-C custody head. Execute the C-locked
canonical or verification role against identical content-addressed inputs.
Attempts are separately invoked and registered; no command can select the better
output. Release requires a successful semantic comparison and closing checkpoint.

### `authorize-enrichment`

Register the user's distinct immutable Stage-2 authorization receipt against the
exact Stage-1 report and E commitment. It does not contact a provider.

### `select-source`

Apply E's content-neutral ranking and tie rule to admitted candidate packets,
then create P. It cannot inspect mechanism-bearing content and cannot legalize
prior access.

### `publish-claims`

Validate atomic claim tiers, scopes, falsifiers, active evidence, and authority
requirements; publish a new immutable claim-registry version or fail closed.

### `publish-report`

Bind the released episode manifest or mechanism result to every attempt,
comparison, commitment, source, atomic claim, uncertainty method, diagnostic,
prior overlap, and required limitation. Missing denominators, null outcomes,
dependence assumptions, generalization scope, positive-unlabeled policy, or
Stage-2 authorship disclosure fails before publication.

### `invalidate`

Apply a source lifecycle transition, traverse registered lineage, and publish the
complete dependent-artifact/claim invalidation set without deleting history.

## Exit Contract

- Exit success means the requested artifact/state transition was atomically
  committed and verified.
- Validation, stale-head, invalid timestamp, unauthorized role, missing source,
  exceeded budget, semantic mismatch, contamination, and lifecycle failures use
  typed nonzero outcomes.
- A retryable execution failure is retryable only when C permits another attempt
  with identical inputs and no result exposure. All other failures are terminal.
- stdout contains non-secret structured summaries. Result-bearing sealed content
  is written only to the authorized artifact destination after release.

## Explicitly Unsupported Commands

The CLI has no provider login, quote, purchase, arbitrary query, universal
backfill, notebook execution, strategy generation, trade submission, live deploy,
credential display, or alternate replay command.
